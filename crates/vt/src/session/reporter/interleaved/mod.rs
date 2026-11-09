//! Interleaved reporter — streams output directly as tasks produce it.
//!
//! In a dry run (`vp run --dry-run`), nothing runs, so there is no output.
//! Each command's line is the one a real run prints, without what the run
//! would do next (`, executing` or `, replaying`), and no blank line follows
//! it.

use std::{cell::RefCell, io::Write, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc};

use vt_path::AbsolutePath;
use vt_plan::{ExecutionItemDisplay, LeafExecutionKind};
use vt_str::Str;

use super::{
    COMMAND_STYLE, ColorSupport, ColorizeExt as _, ExitStatus, GraphExecutionReporter,
    GraphExecutionReporterBuilder, LeafExecutionReporter, PipeWriters, RetryingWriter, StdioConfig,
    StdioSuggestion, format_command_display, format_command_with_cache_status, maybe_strip_writer,
    write_leaf_trailing_output,
};
use crate::session::event::{CacheStatus, CacheUpdateStatus, ExecutionError, RunKind};

pub struct InterleavedReporterBuilder {
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn Write>,
    color_support: ColorSupport,
    run_kind: RunKind,
}

impl InterleavedReporterBuilder {
    /// The reporter's own writes (command lines, error banners) decide
    /// colour-vs-plain at format time via `ColorizeExt`, so `writer` is
    /// stored unwrapped. `color_support` is forwarded to the pipe writers
    /// in `LeafExecutionReporter::start`, where ANSI emitted by child tasks is stripped
    /// for non-terminal sinks.
    pub fn new(
        workspace_path: Arc<AbsolutePath>,
        writer: Box<dyn Write>,
        color_support: ColorSupport,
        run_kind: RunKind,
    ) -> Self {
        Self { workspace_path, writer, color_support, run_kind }
    }
}

impl GraphExecutionReporterBuilder for InterleavedReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        Box::new(InterleavedGraphReporter {
            writer: Rc::new(RefCell::new(self.writer)),
            workspace_path: self.workspace_path,
            color_support: self.color_support,
            run_kind: self.run_kind,
        })
    }
}

struct InterleavedGraphReporter {
    writer: Rc<RefCell<Box<dyn Write>>>,
    workspace_path: Arc<AbsolutePath>,
    color_support: ColorSupport,
    run_kind: RunKind,
}

impl GraphExecutionReporter for InterleavedGraphReporter {
    fn new_leaf_execution(
        &mut self,
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
    ) -> Box<dyn LeafExecutionReporter> {
        let stdio_suggestion = match leaf_kind {
            LeafExecutionKind::Spawn(_) => StdioSuggestion::Inherited,
            LeafExecutionKind::InProcess(_) => StdioSuggestion::Piped,
        };

        Box::new(InterleavedLeafReporter {
            writer: Rc::clone(&self.writer),
            display: display.clone(),
            workspace_path: Arc::clone(&self.workspace_path),
            stdio_suggestion,
            started: false,
            color_support: self.color_support,
            run_kind: self.run_kind,
        })
    }

    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        let mut writer = self.writer.borrow_mut();
        let _ = writer.flush();
        Ok(())
    }
}

struct InterleavedLeafReporter {
    writer: Rc<RefCell<Box<dyn Write>>>,
    display: ExecutionItemDisplay,
    workspace_path: Arc<AbsolutePath>,
    stdio_suggestion: StdioSuggestion,
    started: bool,
    color_support: ColorSupport,
    run_kind: RunKind,
}

impl LeafExecutionReporter for InterleavedLeafReporter {
    fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        let line = format_command_with_cache_status(
            &self.display,
            &self.workspace_path,
            &cache_status,
            self.run_kind,
        );

        self.started = true;

        let mut writer = self.writer.borrow_mut();
        let _ = writer.write_all(line.as_bytes());
        let _ = writer.flush();

        StdioConfig {
            suggestion: self.stdio_suggestion,
            writers: PipeWriters {
                stdout_writer: maybe_strip_writer(
                    Box::new(RetryingWriter::new(std::io::stdout())),
                    self.color_support.stdout,
                ),
                stderr_writer: maybe_strip_writer(
                    Box::new(RetryingWriter::new(std::io::stderr())),
                    self.color_support.stderr,
                ),
            },
        }
    }

    fn finish(
        self: Box<Self>,
        _status: Option<StdExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        let (command_line, blank_line) = match self.run_kind {
            RunKind::Real => (Str::default(), self.started),
            // A dry run prints only the commands' lines. An error that comes
            // without `start()`, such as a failed cache lookup, is printed
            // under its command's line, so it reads as that command's.
            RunKind::Dry if error.is_some() && !self.started => {
                let command = format_command_display(&self.display, &self.workspace_path);
                (vt_str::format!("{}\n", command.style(COMMAND_STYLE)), false)
            }
            RunKind::Dry => (Str::default(), false),
        };
        write_leaf_trailing_output(&self.writer, error, blank_line, command_line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use vt_plan::ExecutionItemKind;

    use super::*;
    use crate::session::{
        cache::CacheMiss,
        event::{CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason},
        reporter::{
            StdioSuggestion,
            test_fixtures::{SharedBuffer, in_process_task, spawn_task, test_path},
        },
    };

    fn leaf_kind(item: &vt_plan::ExecutionItem) -> &LeafExecutionKind {
        match &item.kind {
            ExecutionItemKind::Leaf(kind) => kind,
            ExecutionItemKind::Expanded(_) => panic!("test fixture item must be a Leaf"),
        }
    }

    fn suggestion_for(
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
    ) -> StdioSuggestion {
        let builder = Box::new(InterleavedReporterBuilder::new(
            test_path(),
            Box::new(std::io::sink()),
            ColorSupport::uniform(false),
            RunKind::Real,
        ));
        let mut reporter = builder.build();
        let mut leaf = reporter.new_leaf_execution(display, leaf_kind);
        leaf.start(CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata)).suggestion
    }

    #[test]
    fn spawn_suggests_inherited() {
        let task = spawn_task("build");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item)),
            StdioSuggestion::Inherited
        );
    }

    #[test]
    fn in_process_leaf_suggests_piped() {
        let task = in_process_task("echo");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item)),
            StdioSuggestion::Piped
        );
    }

    #[test]
    fn dry_run_prints_each_line_without_the_next_step_and_an_error_under_its_command() {
        let task = spawn_task("build");
        let item = &task.items[0];
        let output = SharedBuffer::default();
        let mut reporter = Box::new(InterleavedReporterBuilder::new(
            test_path(),
            Box::new(output.clone()),
            ColorSupport::uniform(false),
            RunKind::Dry,
        ))
        .build();
        let not_updated = || CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::DryRun);

        let mut leaf = reporter.new_leaf_execution(&item.execution_item_display, leaf_kind(item));
        leaf.start(CacheStatus::Miss(CacheMiss::NotFound));
        leaf.finish(None, not_updated(), None);

        let leaf = reporter.new_leaf_execution(&item.execution_item_display, leaf_kind(item));
        let error = ExecutionError::Cache {
            kind: CacheErrorKind::Lookup,
            source: anyhow::anyhow!("database is locked"),
        };
        leaf.finish(None, not_updated(), Some(error));
        reporter.finish().unwrap();

        assert_eq!(
            output.text().as_str(),
            "$ build\n$ build\n✗ Cache lookup failed: database is locked\n"
        );
    }
}

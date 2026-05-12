//! Grouped reporter — buffers output per task, prints as a block on completion.

use std::{cell::RefCell, io::Write, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc};

use owo_colors::Style;
use vite_path::AbsolutePath;
use vite_task_plan::{ExecutionItemDisplay, LeafExecutionKind};

use super::{
    ColorSupport, ColorizeExt, ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder,
    LeafExecutionReporter, PipeWriters, StdioConfig, StdioSuggestion,
    format_command_with_cache_status, format_task_label, maybe_strip_writer,
    write_leaf_trailing_output,
};
use crate::session::event::{CacheStatus, CacheUpdateStatus, ExecutionError};

mod writer;

use writer::GroupedWriter;

pub struct GroupedReporterBuilder {
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn Write>,
    color_support: ColorSupport,
}

impl GroupedReporterBuilder {
    /// Grouped mode buffers child output and flushes it through `writer`
    /// at finish time. The pipe writers themselves (see
    /// `LeafExecutionReporter::start`) strip ANSI on the way into the buffer,
    /// so by the time the buffer reaches `writer` it already matches the
    /// terminal's colour capability. `writer` is therefore stored unwrapped.
    pub fn new(
        workspace_path: Arc<AbsolutePath>,
        writer: Box<dyn Write>,
        color_support: ColorSupport,
    ) -> Self {
        Self { workspace_path, writer, color_support }
    }
}

impl GraphExecutionReporterBuilder for GroupedReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        Box::new(GroupedGraphReporter {
            writer: Rc::new(RefCell::new(self.writer)),
            workspace_path: self.workspace_path,
            color_support: self.color_support,
        })
    }
}

struct GroupedGraphReporter {
    writer: Rc<RefCell<Box<dyn Write>>>,
    workspace_path: Arc<AbsolutePath>,
    color_support: ColorSupport,
}

impl GraphExecutionReporter for GroupedGraphReporter {
    fn new_leaf_execution(
        &mut self,
        display: &ExecutionItemDisplay,
        _leaf_kind: &LeafExecutionKind,
    ) -> Box<dyn LeafExecutionReporter> {
        let label = format_task_label(display);
        Box::new(GroupedLeafReporter {
            writer: Rc::clone(&self.writer),
            display: display.clone(),
            workspace_path: Arc::clone(&self.workspace_path),
            label,
            started: false,
            grouped_buffer: None,
            color_support: self.color_support,
        })
    }

    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        let mut writer = self.writer.borrow_mut();
        let _ = writer.flush();
        Ok(())
    }
}

struct GroupedLeafReporter {
    writer: Rc<RefCell<Box<dyn Write>>>,
    display: ExecutionItemDisplay,
    workspace_path: Arc<AbsolutePath>,
    label: vite_str::Str,
    started: bool,
    grouped_buffer: Option<Rc<RefCell<Vec<u8>>>>,
    color_support: ColorSupport,
}

impl LeafExecutionReporter for GroupedLeafReporter {
    fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        let line =
            format_command_with_cache_status(&self.display, &self.workspace_path, &cache_status);

        self.started = true;

        // Print labeled command line immediately (before output is buffered).
        let labeled_line = vite_str::format!("{} {line}", self.label);
        let mut writer = self.writer.borrow_mut();
        let _ = writer.write_all(labeled_line.as_bytes());
        let _ = writer.flush();

        // Create shared buffer for both stdout and stderr.
        let buffer = Rc::new(RefCell::new(Vec::new()));
        self.grouped_buffer = Some(Rc::clone(&buffer));

        StdioConfig {
            suggestion: StdioSuggestion::Piped,
            writers: PipeWriters {
                stdout_writer: maybe_strip_writer(
                    Box::new(GroupedWriter::new(Rc::clone(&buffer))),
                    self.color_support.stdout,
                ),
                stderr_writer: maybe_strip_writer(
                    Box::new(GroupedWriter::new(buffer)),
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
        // Build grouped block: header + buffered output.
        let mut extra = Vec::new();
        if let Some(ref grouped_buffer) = self.grouped_buffer {
            let content = grouped_buffer.borrow();
            if !content.is_empty() {
                let header = vite_str::format!(
                    "{} {} {}\n",
                    "──".style(Style::new().bright_black()),
                    self.label,
                    "──".style(Style::new().bright_black())
                );
                extra.extend_from_slice(header.as_bytes());
                extra.extend_from_slice(&content);
            }
        }

        write_leaf_trailing_output(&self.writer, error, self.started, &extra);
    }
}

#[cfg(test)]
mod tests {
    use vite_task_plan::ExecutionItemKind;

    use super::*;
    use crate::session::{
        event::CacheDisabledReason,
        reporter::{
            StdioSuggestion,
            test_fixtures::{spawn_task, test_path},
        },
    };

    fn leaf_kind(item: &vite_task_plan::ExecutionItem) -> &LeafExecutionKind {
        match &item.kind {
            ExecutionItemKind::Leaf(kind) => kind,
            ExecutionItemKind::Expanded(_) => panic!("test fixture item must be a Leaf"),
        }
    }

    #[test]
    fn always_suggests_piped() {
        let task = spawn_task("build");
        let item = &task.items[0];

        let builder = Box::new(GroupedReporterBuilder::new(
            test_path(),
            Box::new(std::io::sink()),
            ColorSupport::uniform(false),
        ));
        let mut reporter = builder.build();
        let mut leaf = reporter.new_leaf_execution(&item.execution_item_display, leaf_kind(item));
        let stdio_config = leaf.start(CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata));
        assert_eq!(stdio_config.suggestion, StdioSuggestion::Piped);
    }
}

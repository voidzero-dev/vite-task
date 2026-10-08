//! Summary reporter — wraps an inner reporter and adds summary tracking.
//!
//! This is a decorator that intercepts leaf `start()`/`finish()` to track task
//! results, then renders a summary when the graph execution completes. The inner
//! reporter handles all output formatting (interleaved, labeled, grouped).

use std::{
    cell::RefCell,
    io::Write,
    num::NonZeroUsize,
    process::ExitStatus as StdExitStatus,
    rc::Rc,
    sync::{Arc, OnceLock},
};

use vt_path::AbsolutePath;
use vt_plan::{ExecutionItemDisplay, LeafExecutionKind};
use vt_str::Str;

use super::{
    ColorSupport, ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder,
    LeafExecutionReporter, StdioConfig,
};
use crate::session::{
    cache::remote::UploadError,
    event::{CacheStatus, CacheUpdateStatus, ExecutionError},
    reporter::summary::{
        LastRunSummary, SavedError, SpawnOutcome, TaskResult, TaskSummary, format_compact_summary,
        format_full_summary, format_uploads_pending,
    },
};

/// Callback type for persisting the summary (e.g., writing `last-summary.json`).
pub type WriteSummaryFn = Box<dyn FnOnce(&LastRunSummary)>;

/// Builder that wraps an inner builder and adds summary tracking.
pub struct SummaryReporterBuilder {
    inner: Box<dyn GraphExecutionReporterBuilder>,
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn Write>,
    show_details: bool,
    write_summary: Option<WriteSummaryFn>,
    program_name: Str,
}

impl SummaryReporterBuilder {
    /// `writer` is the summary output stream. The wrapped inner builder
    /// owns per-stream stripping of the child-process pipe writers; the
    /// reporter's own summary text picks colour-vs-plain at format time
    /// via `ColorizeExt`, so `writer` is stored unwrapped.
    pub fn new(
        inner: Box<dyn GraphExecutionReporterBuilder>,
        workspace_path: Arc<AbsolutePath>,
        writer: Box<dyn Write>,
        show_details: bool,
        write_summary: Option<WriteSummaryFn>,
        program_name: Str,
        _color_support: ColorSupport,
    ) -> Self {
        Self { inner, workspace_path, writer, show_details, write_summary, program_name }
    }
}

impl GraphExecutionReporterBuilder for SummaryReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        Box::new(SummaryGraphReporter {
            inner: self.inner.build(),
            tasks: Rc::new(RefCell::new(Vec::new())),
            workspace_path: self.workspace_path,
            writer: self.writer,
            show_details: self.show_details,
            write_summary: self.write_summary,
            program_name: self.program_name,
        })
    }
}

/// A finished task's summary, without the result of its upload to the remote
/// cache, which may still be running.
struct RecordedTask {
    summary: TaskSummary,
    /// Where the upload's error is set if it fails. `None` if the task didn't
    /// update the cache.
    upload_error: Option<Arc<OnceLock<UploadError>>>,
}

struct SummaryGraphReporter {
    inner: Box<dyn GraphExecutionReporter>,
    tasks: Rc<RefCell<Vec<RecordedTask>>>,
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn Write>,
    show_details: bool,
    write_summary: Option<WriteSummaryFn>,
    program_name: Str,
}

impl GraphExecutionReporter for SummaryGraphReporter {
    fn new_leaf_execution(
        &mut self,
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
    ) -> Box<dyn LeafExecutionReporter> {
        let inner = self.inner.new_leaf_execution(display, leaf_kind);
        Box::new(SummaryLeafReporter {
            inner,
            tasks: Rc::clone(&self.tasks),
            display: display.clone(),
            workspace_path: Arc::clone(&self.workspace_path),
            cache_status: None,
        })
    }

    fn uploads_pending(&mut self, count: NonZeroUsize) {
        let _ = self.writer.write_all(&format_uploads_pending(count));
        let _ = self.writer.flush();
    }

    /// Called after the uploads to the remote cache finish, so their errors
    /// are in the summary, and in the saved one.
    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        // Let inner reporter finish first (flushes any pending output).
        let inner_result = self.inner.finish();

        let tasks: Vec<TaskSummary> = self
            .tasks
            .take()
            .into_iter()
            .map(|RecordedTask { mut summary, upload_error }| {
                if let Some(error) = upload_error.as_deref().and_then(OnceLock::get) {
                    summary.result.set_upload_error(SavedError::new(error));
                }
                summary
            })
            .collect();

        let has_infra_errors = tasks.iter().any(|t| t.result.error().is_some());

        let failed_exit_codes: Vec<i32> = tasks
            .iter()
            .filter_map(|t| match &t.result {
                TaskResult::Spawned { outcome: SpawnOutcome::Failed { exit_code }, .. } => {
                    Some(exit_code.get())
                }
                _ => None,
            })
            .collect();

        let result = match (has_infra_errors, failed_exit_codes.as_slice()) {
            (false, []) => Ok(()),
            (false, [code]) =>
            {
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "value is clamped to 1..=255, always positive"
                )]
                Err(ExitStatus((*code).clamp(1, 255) as u8))
            }
            _ => Err(ExitStatus::FAILURE),
        };

        let exit_code = match &result {
            Ok(()) => 0u8,
            Err(status) => status.0,
        };

        let summary = LastRunSummary { tasks, exit_code };

        let summary_buf = if self.show_details {
            format_full_summary(&summary)
        } else {
            format_compact_summary(&summary, &self.program_name)
        };

        if let Some(write_summary) = self.write_summary {
            write_summary(&summary);
        }

        // Always flush — even when summary is empty, a preceding spawned process
        // may have written to the same fd via Stdio::inherit().
        {
            let mut writer = self.writer;
            if !summary_buf.is_empty() {
                let _ = writer.write_all(&summary_buf);
            }
            let _ = writer.flush();
        }

        // Use inner result if it failed, otherwise use our computed result.
        inner_result.and(result)
    }
}

/// Leaf reporter wrapper that records task results for the summary.
struct SummaryLeafReporter {
    inner: Box<dyn LeafExecutionReporter>,
    tasks: Rc<RefCell<Vec<RecordedTask>>>,
    display: ExecutionItemDisplay,
    workspace_path: Arc<AbsolutePath>,
    cache_status: Option<CacheStatus>,
}

impl LeafExecutionReporter for SummaryLeafReporter {
    fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        self.cache_status = Some(cache_status.clone());
        self.inner.start(cache_status)
    }

    fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        // Record task summary before forwarding to inner.
        let saved_error = error.as_ref().map(|error| SavedError::new(error));

        if let Some(ref cache_status) = self.cache_status {
            let cwd_relative =
                if let Ok(Some(rel)) = self.display.cwd.strip_prefix(&self.workspace_path) {
                    Str::from(rel.as_str())
                } else {
                    Str::default()
                };

            let summary = TaskSummary {
                package_name: self.display.task_display.package_name.clone(),
                task_name: self.display.task_display.task_name.clone(),
                command: self.display.command.clone(),
                cwd: cwd_relative,
                result: TaskResult::from_execution(
                    cache_status,
                    status,
                    saved_error.as_ref(),
                    &cache_update_status,
                    &self.display.task_display.package_path,
                    &self.workspace_path,
                ),
            };
            let upload_error = match &cache_update_status {
                CacheUpdateStatus::Updated { upload_error } => Some(Arc::clone(upload_error)),
                CacheUpdateStatus::NotUpdated(_) => None,
            };

            self.tasks.borrow_mut().push(RecordedTask { summary, upload_error });
        }

        self.inner.finish(status, cache_update_status, error);
    }
}

#[cfg(test)]
mod tests {
    use vt_plan::ExecutionItemKind;

    use super::*;
    use crate::session::{
        cache::CacheMiss,
        reporter::{
            InterleavedReporterBuilder,
            test_fixtures::{spawn_task, test_path},
        },
    };

    /// A writer whose output stays readable after it's moved into a reporter.
    #[derive(Clone, Default)]
    struct SharedBuffer(Rc<RefCell<Vec<u8>>>);

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuffer {
        fn text(&self) -> Str {
            let bytes = self.0.borrow();
            let text = std::str::from_utf8(&bytes).unwrap();
            vt_str::format!("{}", anstream::adapter::strip_str(text))
        }
    }

    #[test]
    fn upload_error_set_after_the_task_finishes_is_in_the_summary() {
        let task = spawn_task("build");
        let item = &task.items[0];
        let ExecutionItemKind::Leaf(leaf_kind) = &item.kind else {
            panic!("test fixture item must be a Leaf");
        };
        let output = SharedBuffer::default();
        let saved = SharedBuffer::default();
        let write_summary: WriteSummaryFn = Box::new({
            let mut saved = saved.clone();
            move |summary| saved.write_all(&format_compact_summary(summary, "vp")).unwrap()
        });
        let mut reporter = Box::new(SummaryReporterBuilder::new(
            Box::new(InterleavedReporterBuilder::new(
                test_path(),
                Box::new(std::io::sink()),
                ColorSupport::uniform(false),
            )),
            test_path(),
            Box::new(output.clone()),
            false,
            Some(write_summary),
            Str::from("vp"),
            ColorSupport::uniform(false),
        ))
        .build();

        let upload_error = Arc::new(OnceLock::new());
        let mut leaf = reporter.new_leaf_execution(&item.execution_item_display, leaf_kind);
        leaf.start(CacheStatus::Miss(CacheMiss::NotFound));
        leaf.finish(
            Some(StdExitStatus::default()),
            CacheUpdateStatus::Updated { upload_error: Arc::clone(&upload_error) },
            None,
        );
        reporter.uploads_pending(NonZeroUsize::MIN);
        upload_error.set(UploadError::Interrupted).unwrap();
        reporter.finish().unwrap();

        let summary = "---\nvp run: pkg#build not uploaded to the remote cache: interrupted. \
                       (Run `vp run --last-details` for full details)\n";
        assert_eq!(saved.text().as_str(), summary);
        assert_eq!(
            output.text().as_str(),
            vt_str::format!("Waiting for 1 remote cache upload to finish...\n{summary}").as_str()
        );
    }
}

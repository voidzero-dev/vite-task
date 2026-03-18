//! Labeled reporter family — graph-aware reporter with aggregation and summary.
//!
//! Provides the full reporter lifecycle:
//! - [`LabeledReporterBuilder`] → [`LabeledGraphReporter`] → [`LabeledLeafReporter`]
//!
//! Tracks statistics across multiple leaf executions, prints command lines with cache
//! status indicators, and renders a summary with per-task details at the end.

use std::{cell::RefCell, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc};

use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_plan::{ExecutionItemDisplay, LeafExecutionKind};

use super::{
    ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
    StdioConfig, StdioSuggestion, format_command_with_cache_status, format_error_message,
};
use crate::session::{
    event::{CacheStatus, CacheUpdateStatus, ExecutionError},
    reporter::summary::{
        LastRunSummary, SavedExecutionError, SpawnOutcome, TaskResult, TaskSummary,
        format_compact_summary, format_full_summary,
    },
};

/// Callback type for persisting the summary (e.g., writing `last-summary.json`).
type WriteSummaryFn = Box<dyn FnOnce(&LastRunSummary)>;

/// Mutable state shared between [`LabeledGraphReporter`] and its [`LabeledLeafReporter`] instances
/// via `Rc<RefCell<...>>`.
///
/// This is safe because execution is single-threaded and sequential — only one leaf
/// reporter is active at a time.
struct SharedReporterState {
    tasks: Vec<TaskSummary>,
}

/// Builder for the labeled graph reporter.
///
/// Created by the caller before execution, then transitioned to [`LabeledGraphReporter`]
/// by calling `build()` with the execution graph.
///
/// # Output Modes
///
/// ## Compact Summary (default)
/// - Single task + not cache hit → no summary at all
/// - Single task + cache hit → thin line + "vp run: cache hit, {duration} saved."
/// - Multi-task → thin line + one-liner with stats
///
/// ## Full Summary (`--verbose`)
/// - Shows full Statistics, Performance, and Task Details sections
pub struct LabeledReporterBuilder {
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn AsyncWrite + Unpin>,
    /// Whether to render the full detailed summary (`--verbose` flag).
    show_details: bool,
    /// Callback to persist the summary (e.g., write `last-summary.json`).
    /// `None` when persistence is not needed (e.g., nested script execution, tests).
    write_summary: Option<WriteSummaryFn>,
    program_name: Str,
}

impl LabeledReporterBuilder {
    /// Create a new labeled reporter builder.
    ///
    /// - `workspace_path`: The workspace root, used to compute relative cwds in display.
    /// - `writer`: Async writer for reporter display output.
    /// - `show_details`: Whether to render the full detailed summary.
    /// - `write_summary`: Callback to persist the summary, or `None` to skip.
    /// - `program_name`: The CLI binary name (e.g. `"vt"`) used in summary output.
    pub fn new(
        workspace_path: Arc<AbsolutePath>,
        writer: Box<dyn AsyncWrite + Unpin>,
        show_details: bool,
        write_summary: Option<WriteSummaryFn>,
        program_name: Str,
    ) -> Self {
        Self { workspace_path, writer, show_details, write_summary, program_name }
    }
}

impl GraphExecutionReporterBuilder for LabeledReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        let writer = Rc::new(RefCell::new(self.writer));
        Box::new(LabeledGraphReporter {
            shared: Rc::new(RefCell::new(SharedReporterState { tasks: Vec::new() })),
            writer,
            workspace_path: self.workspace_path,
            show_details: self.show_details,
            write_summary: self.write_summary,
            program_name: self.program_name,
        })
    }
}

/// Graph-level reporter that tracks multiple leaf executions and prints a summary.
///
/// Creates [`LabeledLeafReporter`] instances for each leaf execution. The leaf reporters
/// share mutable state with this reporter via `Rc<RefCell<SharedReporterState>>`.
pub struct LabeledGraphReporter {
    shared: Rc<RefCell<SharedReporterState>>,
    writer: Rc<RefCell<Box<dyn AsyncWrite + Unpin>>>,
    workspace_path: Arc<AbsolutePath>,
    show_details: bool,
    write_summary: Option<WriteSummaryFn>,
    program_name: Str,
}

#[async_trait::async_trait(?Send)]
#[expect(
    clippy::await_holding_refcell_ref,
    reason = "writer RefCell borrow across await is safe: reporter is !Send, single-threaded, \
              and finish() is called once after all leaf reporters are dropped"
)]
impl GraphExecutionReporter for LabeledGraphReporter {
    fn new_leaf_execution(
        &mut self,
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
        all_ancestors_single_node: bool,
    ) -> Box<dyn LeafExecutionReporter> {
        let display = display.clone();
        let stdio_suggestion = match leaf_kind {
            LeafExecutionKind::Spawn(_) if all_ancestors_single_node => StdioSuggestion::Inherited,
            _ => StdioSuggestion::Piped,
        };

        Box::new(LabeledLeafReporter {
            shared: Rc::clone(&self.shared),
            writer: Rc::clone(&self.writer),
            display,
            workspace_path: Arc::clone(&self.workspace_path),
            stdio_suggestion,
            cache_status: None,
        })
    }

    async fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        // Take tasks from shared state — all leaf reporters have been dropped by now.
        let tasks = {
            let mut shared = self.shared.borrow_mut();
            std::mem::take(&mut shared.tasks)
        };

        // Compute exit status from the collected task results.
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

        // Build summary from collected tasks.
        let summary = LastRunSummary { tasks, exit_code };

        // Render summary based on mode.
        let summary_buf = if self.show_details {
            format_full_summary(&summary)
        } else {
            format_compact_summary(&summary, &self.program_name)
        };

        // Persist summary via callback (best-effort, callback handles errors).
        if let Some(write_summary) = self.write_summary {
            write_summary(&summary);
        }

        // Write the summary buffer asynchronously.
        // Always flush the writer — even when the summary is empty, a preceding
        // spawned process may have written to the same fd via Stdio::inherit()
        // and the data must be flushed before the caller reads the output.
        {
            let mut writer = self.writer.borrow_mut();
            if !summary_buf.is_empty() {
                let _ = writer.write_all(&summary_buf).await;
            }
            let _ = writer.flush().await;
        }

        result
    }
}

/// Leaf-level reporter created by [`LabeledGraphReporter::new_leaf_execution`].
///
/// Writes display output in real-time to the shared async writer and builds
/// [`TaskSummary`] entries that are pushed to [`SharedReporterState`] on completion.
struct LabeledLeafReporter {
    shared: Rc<RefCell<SharedReporterState>>,
    writer: Rc<RefCell<Box<dyn AsyncWrite + Unpin>>>,
    /// Display info for this execution, looked up from the graph via the path.
    display: ExecutionItemDisplay,
    workspace_path: Arc<AbsolutePath>,
    /// Stdio suggestion precomputed from this leaf's graph path.
    stdio_suggestion: StdioSuggestion,
    /// Cache status, set at `start()` time. `None` means `start()` was never called
    /// (e.g., cache lookup failure). Consumed in `finish()` to build [`TaskSummary`].
    cache_status: Option<CacheStatus>,
}

#[async_trait::async_trait(?Send)]
#[expect(
    clippy::await_holding_refcell_ref,
    reason = "writer RefCell borrow across await is safe: reporter is !Send, single-threaded, \
              and only one leaf is active at a time (no re-entrant access during write_all)"
)]
impl LeafExecutionReporter for LabeledLeafReporter {
    async fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        // Format command line with cache status before storing it.
        let line =
            format_command_with_cache_status(&self.display, &self.workspace_path, &cache_status);

        self.cache_status = Some(cache_status);

        let mut writer = self.writer.borrow_mut();
        let _ = writer.write_all(line.as_bytes()).await;
        let _ = writer.flush().await;

        StdioConfig {
            suggestion: self.stdio_suggestion,
            stdout_writer: Box::new(tokio::io::stdout()),
            stderr_writer: Box::new(tokio::io::stderr()),
        }
    }

    async fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        // Convert error before consuming it (need the original for display formatting).
        let saved_error = error.as_ref().map(SavedExecutionError::from_execution_error);
        let error_display: Option<Str> =
            error.map(|e| vite_str::format!("{:#}", anyhow::Error::from(e)));

        // Destructure self to avoid partial-move issues with Box<Self>.
        let Self { shared, writer, display, workspace_path, cache_status, .. } = *self;
        let started = cache_status.is_some();

        // Build TaskSummary and push to shared state if start() was called.
        if let Some(cache_status) = cache_status {
            let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(&workspace_path) {
                Str::from(rel.as_str())
            } else {
                Str::default()
            };

            let task_summary = TaskSummary {
                package_name: display.task_display.package_name.clone(),
                task_name: display.task_display.task_name.clone(),
                command: display.command.clone(),
                cwd: cwd_relative,
                result: TaskResult::from_execution(
                    &cache_status,
                    status,
                    saved_error.as_ref(),
                    &cache_update_status,
                ),
            };

            shared.borrow_mut().tasks.push(task_summary);
        }

        // Build all display output into a buffer, then write once asynchronously.
        let mut buf = Vec::new();

        if let Some(ref message) = error_display {
            buf.extend_from_slice(format_error_message(message).as_bytes());
        }

        // Add a trailing newline after each task's output for readability.
        // Skip if start() was never called (e.g. cache lookup failure) — there's
        // no task output to separate.
        if started {
            buf.push(b'\n');
        }

        if !buf.is_empty() {
            let mut writer = writer.borrow_mut();
            let _ = writer.write_all(&buf).await;
            let _ = writer.flush().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use vite_task_plan::ExecutionItemKind;

    use super::*;
    use crate::session::{
        event::CacheDisabledReason,
        reporter::{
            LeafExecutionReporter, StdioSuggestion,
            test_fixtures::{in_process_task, spawn_task, test_path},
        },
    };

    /// Extract the `LeafExecutionKind` from a test fixture item.
    /// Panics if the item is not a leaf (test fixtures always produce leaves).
    fn leaf_kind(item: &vite_task_plan::ExecutionItem) -> &LeafExecutionKind {
        match &item.kind {
            ExecutionItemKind::Leaf(kind) => kind,
            ExecutionItemKind::Expanded(_) => panic!("test fixture item must be a Leaf"),
        }
    }

    fn build_labeled_leaf(
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
        all_ancestors_single_node: bool,
    ) -> Box<dyn LeafExecutionReporter> {
        let builder = Box::new(LabeledReporterBuilder::new(
            test_path(),
            Box::new(tokio::io::sink()),
            false,
            None,
            Str::from("vt"),
        ));
        let mut reporter = builder.build();
        reporter.new_leaf_execution(display, leaf_kind, all_ancestors_single_node)
    }

    async fn suggestion_for(
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
        all_ancestors_single_node: bool,
    ) -> StdioSuggestion {
        let mut leaf = build_labeled_leaf(display, leaf_kind, all_ancestors_single_node);
        let stdio_config =
            leaf.start(CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata)).await;
        stdio_config.suggestion
    }

    #[tokio::test]
    async fn spawn_with_all_single_node_ancestors_suggests_inherited() {
        let task = spawn_task("build");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item), true).await,
            StdioSuggestion::Inherited
        );
    }

    #[tokio::test]
    async fn spawn_without_all_single_node_ancestors_suggests_piped() {
        let task = spawn_task("build");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item), false).await,
            StdioSuggestion::Piped
        );
    }

    #[tokio::test]
    async fn in_process_leaf_suggests_piped_even_with_single_node_ancestors() {
        let task = in_process_task("echo");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item), true).await,
            StdioSuggestion::Piped
        );
    }

    #[tokio::test]
    async fn in_process_leaf_suggests_piped_without_single_node_ancestors() {
        let task = in_process_task("echo");
        let item = &task.items[0];
        assert_eq!(
            suggestion_for(&item.execution_item_display, leaf_kind(item), false).await,
            StdioSuggestion::Piped
        );
    }
}

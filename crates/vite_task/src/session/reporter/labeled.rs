//! Labeled reporter family — graph-aware reporter with aggregation and summary.
//!
//! Provides the full reporter lifecycle:
//! - [`LabeledReporterBuilder`] → [`LabeledGraphReporter`] → [`LabeledLeafReporter`]
//!
//! Tracks statistics across multiple leaf executions, prints command lines with cache
//! status indicators, and renders a summary with per-task details at the end.

use std::{cell::RefCell, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc};

use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;
use vite_task_plan::{ExecutionItemDisplay, LeafExecutionKind};

use super::{
    ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
    StdioConfig, StdioSuggestion, format_command_with_cache_status, format_error_message,
};
use crate::session::{
    event::{CacheStatus, CacheUpdateStatus, ExecutionError, exit_status_to_code},
    reporter::summary::{
        LastRunSummary, SavedExecutionError, format_compact_summary, format_full_summary,
    },
};

/// Information tracked for each leaf execution, used to build the summary.
#[derive(Debug)]
struct ExecutionInfo {
    display: ExecutionItemDisplay,
    /// Cache status, determined at `start()`.
    cache_status: CacheStatus,
    /// Exit status from the process. `None` means no process was spawned (cache hit or in-process).
    exit_status: Option<StdExitStatus>,
    /// Execution error, converted to the serializable form for the summary.
    saved_error: Option<SavedExecutionError>,
}

/// Running statistics updated as leaf executions complete.
#[derive(Default)]
struct ExecutionStats {
    failed: usize,
}

/// Mutable state shared between [`LabeledGraphReporter`] and its [`LabeledLeafReporter`] instances
/// via `Rc<RefCell<...>>`.
///
/// This is safe because execution is single-threaded and sequential — only one leaf
/// reporter is active at a time.
struct SharedReporterState {
    executions: Vec<ExecutionInfo>,
    stats: ExecutionStats,
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
/// - Single task + cache hit → thin line + "[vp run] cache hit, {duration} saved."
/// - Multi-task → thin line + one-liner with stats
///
/// ## Full Summary (`--details`)
/// - Shows full Statistics, Performance, and Task Details sections
pub struct LabeledReporterBuilder {
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn AsyncWrite + Unpin>,
    /// Whether to render the full detailed summary (`--details` flag).
    show_details: bool,
    /// Path to write `last-summary.json`. `None` when persistence is not needed
    /// (e.g., nested script execution).
    summary_file_path: Option<AbsolutePathBuf>,
}

impl LabeledReporterBuilder {
    /// Create a new labeled reporter builder.
    ///
    /// - `workspace_path`: The workspace root, used to compute relative cwds in display.
    /// - `writer`: Async writer for reporter display output.
    /// - `show_details`: Whether to render the full detailed summary.
    /// - `summary_file_path`: Where to persist `last-summary.json`, or `None` to skip.
    pub fn new(
        workspace_path: Arc<AbsolutePath>,
        writer: Box<dyn AsyncWrite + Unpin>,
        show_details: bool,
        summary_file_path: Option<AbsolutePathBuf>,
    ) -> Self {
        Self { workspace_path, writer, show_details, summary_file_path }
    }
}

impl GraphExecutionReporterBuilder for LabeledReporterBuilder {
    fn build(self: Box<Self>) -> Box<dyn GraphExecutionReporter> {
        let writer = Rc::new(RefCell::new(self.writer));
        Box::new(LabeledGraphReporter {
            shared: Rc::new(RefCell::new(SharedReporterState {
                executions: Vec::new(),
                stats: ExecutionStats::default(),
            })),
            writer,
            workspace_path: self.workspace_path,
            show_details: self.show_details,
            summary_file_path: self.summary_file_path,
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
    summary_file_path: Option<AbsolutePathBuf>,
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
            started: false,
        })
    }

    async fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        // Borrow shared state synchronously to build the summary and compute
        // the exit result. The borrow is dropped before any async writes.
        let (summary_buf, result, exit_code) = {
            let shared = self.shared.borrow();

            // Build LastRunSummary from execution data
            let executions: Vec<_> = shared
                .executions
                .iter()
                .map(|exec| {
                    (&exec.display, &exec.cache_status, exec.exit_status, exec.saved_error.as_ref())
                })
                .collect();

            // Determine exit code (same logic as before)
            let has_infra_errors = shared.executions.iter().any(|exec| exec.saved_error.is_some());

            let failed_exit_codes: Vec<i32> = shared
                .executions
                .iter()
                .filter_map(|exec| exec.exit_status.as_ref())
                .filter(|status| !status.success())
                .map(|status| exit_status_to_code(*status))
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

            // Build LastRunSummary from the execution data
            let summary =
                LastRunSummary::from_executions(&executions, &self.workspace_path, exit_code);

            // Render summary based on mode
            let summary_buf = if self.show_details {
                format_full_summary(&summary)
            } else {
                format_compact_summary(&summary)
            };

            // Save summary to file (best-effort, log failures)
            if let Some(ref path) = self.summary_file_path
                && let Err(err) = summary.write_atomic(path)
            {
                tracing::warn!("Failed to write summary to {:?}: {err}", path);
            }

            (summary_buf, result, exit_code)
        };
        // shared borrow dropped here
        let _ = exit_code; // used only in the block above

        // Write the summary buffer asynchronously
        if !summary_buf.is_empty() {
            let mut writer = self.writer.borrow_mut();
            let _ = writer.write_all(&summary_buf).await;
            let _ = writer.flush().await;
        }

        result
    }
}

/// Leaf-level reporter created by [`LabeledGraphReporter::new_leaf_execution`].
///
/// Writes display output in real-time to the shared async writer and updates shared
/// stats/errors via `Rc<RefCell<SharedReporterState>>`.
struct LabeledLeafReporter {
    shared: Rc<RefCell<SharedReporterState>>,
    writer: Rc<RefCell<Box<dyn AsyncWrite + Unpin>>>,
    /// Display info for this execution, looked up from the graph via the path.
    display: ExecutionItemDisplay,
    workspace_path: Arc<AbsolutePath>,
    /// Stdio suggestion precomputed from this leaf's graph path.
    stdio_suggestion: StdioSuggestion,
    /// Whether `start()` has been called. Used to determine if stats should be updated
    /// in `finish()` and whether to push an `ExecutionInfo` entry.
    started: bool,
}

#[async_trait::async_trait(?Send)]
#[expect(
    clippy::await_holding_refcell_ref,
    reason = "writer RefCell borrow across await is safe: reporter is !Send, single-threaded, \
              and only one leaf is active at a time (no re-entrant access during write_all)"
)]
impl LeafExecutionReporter for LabeledLeafReporter {
    async fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        self.started = true;

        // Update shared state synchronously, then drop the borrow before any async writes.
        {
            let mut shared = self.shared.borrow_mut();

            // Store execution info for the summary
            shared.executions.push(ExecutionInfo {
                display: self.display.clone(),
                cache_status,
                exit_status: None,
                saved_error: None,
            });
        }
        // shared borrow dropped here

        // Format command line with cache status (sync), then write asynchronously.
        // The shared borrow to read cache_status is brief and dropped before the await.
        let line = {
            let shared = self.shared.borrow();
            let cache_status = &shared.executions.last().unwrap().cache_status;
            format_command_with_cache_status(&self.display, &self.workspace_path, cache_status)
        };
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
        _cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        // Convert the execution error to its serializable form before borrowing shared state
        let saved_error = error.as_ref().map(SavedExecutionError::from_execution_error);
        let has_error = saved_error.is_some();

        // Format the error message for display (using the original error with full anyhow chain)
        let error_display: Option<Str> =
            error.map(|e| vite_str::format!("{:#}", anyhow::Error::from(e)));

        // Update shared state synchronously, then drop the borrow before any async writes.
        {
            let mut shared = self.shared.borrow_mut();

            // Handle errors — update execution info and stats.
            if saved_error.is_some() {
                // Update the execution info if start() was called (an entry was pushed).
                // Without the `self.started` guard, `last_mut()` would return a
                // *different* execution's entry, corrupting its error.
                if self.started
                    && let Some(exec) = shared.executions.last_mut()
                {
                    exec.saved_error = saved_error;
                }

                shared.stats.failed += 1;
            }

            // Update failure statistics for non-zero exit status (not an error, just a failed task)
            // None means success (cache hit or in-process), Some checks the actual exit status
            if !has_error && status.is_some_and(|s| !s.success()) {
                shared.stats.failed += 1;
            }

            // Update execution info with exit status (if start() was called and an entry exists)
            if self.started
                && let Some(exec) = shared.executions.last_mut()
            {
                exec.exit_status = status;
            }
        }
        // shared borrow dropped here

        // Build all display output into a buffer (sync), then write once asynchronously.
        let mut buf = Vec::new();

        if let Some(ref message) = error_display {
            buf.extend_from_slice(format_error_message(message).as_bytes());
        }

        // Add a trailing newline after each task's output for readability.
        // Skip if start() was never called (e.g. cache lookup failure) — there's
        // no task output to separate.
        if self.started {
            buf.push(b'\n');
        }

        if !buf.is_empty() {
            let mut writer = self.writer.borrow_mut();
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
        ));
        let mut reporter = builder.build();
        reporter.new_leaf_execution(display, leaf_kind, all_ancestors_single_node)
    }

    #[expect(
        clippy::future_not_send,
        reason = "LeafExecutionReporter futures are !Send in single-threaded reporter tests"
    )]
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

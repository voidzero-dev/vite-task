//! Labeled reporter family — graph-aware reporter with aggregation and summary.
//!
//! Provides the full reporter lifecycle:
//! - [`LabeledReporterBuilder`] → [`LabeledGraphReporter`] → [`LabeledLeafReporter`]
//!
//! Tracks statistics across multiple leaf executions, prints command lines with cache
//! status indicators, and renders a summary with per-task details at the end.

use std::{cell::RefCell, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc, time::Duration};

use owo_colors::Style;
use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_plan::{ExecutionItemDisplay, LeafExecutionKind};

use super::{
    CACHE_MISS_STYLE, COMMAND_STYLE, ColorizeExt, ExitStatus, GraphExecutionReporter,
    GraphExecutionReporterBuilder, LeafExecutionReporter, StdioConfig, StdioSuggestion,
    format_command_display, format_command_with_cache_status, format_error_message,
};
use crate::session::{
    cache::format_cache_status_summary,
    event::{CacheStatus, CacheUpdateStatus, ExecutionError, exit_status_to_code},
};

/// Information tracked for each leaf execution, used in the final summary.
#[derive(Debug)]
struct ExecutionInfo {
    display: ExecutionItemDisplay,
    /// Cache status, determined at `start()`.
    cache_status: CacheStatus,
    /// Exit status from the process. `None` means no process was spawned (cache hit or in-process).
    exit_status: Option<StdExitStatus>,
    /// Error message, set on error.
    error_message: Option<Str>,
}

/// Running statistics updated as leaf executions complete.
#[derive(Default)]
struct ExecutionStats {
    cache_hits: usize,
    cache_misses: usize,
    cache_disabled: usize,
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
/// ## Normal Mode (default)
/// - Prints command lines with cache status indicators during execution
/// - Shows full summary with Statistics and Task Details at the end
///
/// ## Simplified Summary for Single Tasks
/// - When a single task with display info is executed:
///   - Skips full summary (no Statistics/Task Details sections)
///   - Shows only cache status inline
///   - Results in clean output showing just the command's stdout/stderr
pub struct LabeledReporterBuilder {
    workspace_path: Arc<AbsolutePath>,
    writer: Box<dyn AsyncWrite + Unpin>,
}

impl LabeledReporterBuilder {
    /// Create a new labeled reporter builder.
    ///
    /// - `workspace_path`: The workspace root, used to compute relative cwds in display.
    /// - `writer`: Async writer for reporter display output.
    pub fn new(workspace_path: Arc<AbsolutePath>, writer: Box<dyn AsyncWrite + Unpin>) -> Self {
        Self { workspace_path, writer }
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
        // Borrow shared state synchronously to build the summary buffer and compute
        // the exit result. The borrow is dropped before any async writes.
        let (summary_buf, result) = {
            let shared = self.shared.borrow();

            let summary_buf =
                format_summary(&shared.executions, &shared.stats, &self.workspace_path);

            // Determine exit code based on failed tasks and infrastructure errors:
            // - Infrastructure errors (cache lookup, spawn failure) have error_message set
            //   but no meaningful exit_status.
            // - Process failures have a non-zero exit_status.
            //
            // Rules:
            // 1. No failures at all → Ok(())
            // 2. Exactly one process failure, no infra errors → use that task's exit code
            // 3. Any infra errors, or multiple failures → Err(1)
            let has_infra_errors =
                shared.executions.iter().any(|exec| exec.error_message.is_some());

            let failed_exit_codes: Vec<i32> = shared
                .executions
                .iter()
                .filter_map(|exec| exec.exit_status.as_ref())
                .filter(|status| !status.success())
                .map(|status| exit_status_to_code(*status))
                .collect();

            let result = match (has_infra_errors, failed_exit_codes.as_slice()) {
                (false, []) => Ok(()),
                (false, [code]) => {
                    // Return the single failed task's exit code (clamped to u8 range)
                    #[expect(
                        clippy::cast_sign_loss,
                        reason = "value is clamped to 1..=255, always positive"
                    )]
                    Err(ExitStatus((*code).clamp(1, 255) as u8))
                }
                _ => Err(ExitStatus::FAILURE),
            };

            (summary_buf, result)
        };
        // shared borrow dropped here

        // Write the summary buffer asynchronously
        {
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

            // Update statistics based on cache status
            match &cache_status {
                CacheStatus::Hit { .. } => shared.stats.cache_hits += 1,
                CacheStatus::Miss(_) => shared.stats.cache_misses += 1,
                CacheStatus::Disabled(_) => shared.stats.cache_disabled += 1,
            }

            // Store execution info for the summary
            shared.executions.push(ExecutionInfo {
                display: self.display.clone(),
                cache_status,
                exit_status: None,
                error_message: None,
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
        // Format error message up front (before borrowing shared state)
        let error_message: Option<Str> =
            error.map(|e| vite_str::format!("{:#}", anyhow::Error::from(e)));
        let has_error = error_message.is_some();

        // Update shared state synchronously, then drop the borrow before any async writes.
        {
            let mut shared = self.shared.borrow_mut();

            // Handle errors — update execution info and stats.
            // Error message is formatted using anyhow's `{:#}` formatter
            // (joins cause chain with `: ` separators).
            if let Some(ref message) = error_message {
                // Update the execution info if start() was called (an entry was pushed).
                // Without the `self.started` guard, `last_mut()` would return a
                // *different* execution's entry, corrupting its error_message.
                if self.started
                    && let Some(exec) = shared.executions.last_mut()
                {
                    exec.error_message = Some(message.clone());
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

        if let Some(ref message) = error_message {
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

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Summary printing
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Format the full execution summary into a byte buffer.
///
/// Called by [`LabeledGraphReporter::finish`] after all tasks have executed.
/// The caller writes the returned buffer to the async writer.
///
/// Building the summary synchronously into a `Vec<u8>` buffer avoids holding
/// `RefCell` borrows across async write points, and ensures atomic output.
#[expect(
    clippy::too_many_lines,
    reason = "summary formatting is inherently verbose with many write calls"
)]
fn format_summary(
    executions: &[ExecutionInfo],
    stats: &ExecutionStats,
    workspace_path: &AbsolutePath,
) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::new();

    let total = executions.len();
    let cache_hits = stats.cache_hits;
    let cache_misses = stats.cache_misses;
    let cache_disabled = stats.cache_disabled;
    let failed = stats.failed;

    // Print summary header with decorative line
    // Note: leaf finish already adds a trailing newline after each task's output
    // Add an extra blank line before the summary for visual separation
    let _ = writeln!(buf);
    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(
        buf,
        "{}",
        "    Vite+ Task Runner • Execution Summary".style(Style::new().bold().bright_white())
    );
    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(buf);

    // Print statistics
    let cache_disabled_str = if cache_disabled > 0 {
        Str::from(
            vite_str::format!("• {cache_disabled} cache disabled")
                .style(Style::new().bright_black())
                .to_string(),
        )
    } else {
        Str::default()
    };

    let failed_str = if failed > 0 {
        Str::from(vite_str::format!("• {failed} failed").style(Style::new().red()).to_string())
    } else {
        Str::default()
    };

    // Build statistics line, only including non-empty parts
    let _ = write!(
        buf,
        "{}  {} {} {}",
        "Statistics:".style(Style::new().bold()),
        vite_str::format!(" {total} tasks").style(Style::new().bright_white()),
        vite_str::format!("• {cache_hits} cache hits").style(Style::new().green()),
        vite_str::format!("• {cache_misses} cache misses").style(CACHE_MISS_STYLE),
    );
    if !cache_disabled_str.is_empty() {
        let _ = write!(buf, " {cache_disabled_str}");
    }
    if !failed_str.is_empty() {
        let _ = write!(buf, " {failed_str}");
    }
    let _ = writeln!(buf);

    // Calculate cache hit rate
    let cache_rate = if total > 0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "percentage is always 0..=100, fits in u32"
        )]
        #[expect(clippy::cast_sign_loss, reason = "percentage is always non-negative")]
        #[expect(
            clippy::cast_precision_loss,
            reason = "acceptable precision loss for display percentage"
        )]
        {
            (f64::from(cache_hits as u32) / total as f64 * 100.0) as u32
        }
    } else {
        0
    };

    // Calculate total time saved from cache hits
    let total_saved: Duration = executions
        .iter()
        .filter_map(|exec| {
            if let CacheStatus::Hit { replayed_duration } = &exec.cache_status {
                Some(*replayed_duration)
            } else {
                None
            }
        })
        .sum();

    let _ = write!(
        buf,
        "{}  {} cache hit rate",
        "Performance:".style(Style::new().bold()),
        format_args!("{cache_rate}%").style(if cache_rate >= 75 {
            Style::new().green().bold()
        } else if cache_rate >= 50 {
            CACHE_MISS_STYLE
        } else {
            Style::new().red()
        })
    );

    if total_saved > Duration::ZERO {
        let _ =
            write!(buf, ", {:.2?} saved in total", total_saved.style(Style::new().green().bold()));
    }
    let _ = writeln!(buf);
    let _ = writeln!(buf);

    // Detailed task results
    let _ = writeln!(buf, "{}", "Task Details:".style(Style::new().bold()));
    let _ = writeln!(
        buf,
        "{}",
        "────────────────────────────────────────────────".style(Style::new().bright_black())
    );

    for (idx, exec) in executions.iter().enumerate() {
        let display = &exec.display;

        let task_display = &display.task_display;

        // Task name and index
        let _ = write!(
            buf,
            "  {} {}",
            vite_str::format!("[{}]", idx + 1).style(Style::new().bright_black()),
            task_display.to_string().style(Style::new().bright_white().bold())
        );

        // Command with cwd prefix
        let command_display = format_command_display(display, workspace_path);
        let _ = write!(buf, ": {}", command_display.style(COMMAND_STYLE));

        // Execution result icon
        // None means success (cache hit or in-process), Some checks actual status
        match &exec.exit_status {
            None => {
                let _ = write!(buf, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) if exit_status.success() => {
                let _ = write!(buf, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) => {
                let code = exit_status_to_code(*exit_status);
                let _ = write!(
                    buf,
                    " {} {}",
                    "✗".style(Style::new().red().bold()),
                    vite_str::format!("(exit code: {code})").style(Style::new().red())
                );
            }
        }
        let _ = writeln!(buf);

        // Cache status details — use display module for plain text, apply styling here
        let cache_summary = format_cache_status_summary(&exec.cache_status);
        let styled_summary = match &exec.cache_status {
            CacheStatus::Hit { .. } => cache_summary.style(Style::new().green()),
            CacheStatus::Miss(_) => cache_summary.style(CACHE_MISS_STYLE),
            CacheStatus::Disabled(_) => cache_summary.style(Style::new().bright_black()),
        };
        let _ = writeln!(buf, "      {styled_summary}");

        // Error message if present
        if let Some(ref error_msg) = exec.error_message {
            let _ = writeln!(
                buf,
                "      {} {}",
                "✗ Error:".style(Style::new().red().bold()),
                error_msg.style(Style::new().red())
            );
        }

        // Add spacing between tasks except for the last one
        if idx < executions.len() - 1 {
            let _ = writeln!(
                buf,
                "  {}",
                "·······················································"
                    .style(Style::new().bright_black())
            );
        }
    }

    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );

    buf
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
        let builder =
            Box::new(LabeledReporterBuilder::new(test_path(), Box::new(tokio::io::sink())));
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

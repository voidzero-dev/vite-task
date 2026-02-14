//! Labeled reporter family — graph-aware reporter with aggregation and summary.
//!
//! Provides the full reporter lifecycle:
//! - [`LabeledReporterBuilder`] → [`LabeledGraphReporter`] → [`LabeledLeafReporter`]
//!
//! Tracks statistics across multiple leaf executions, prints command lines with cache
//! status indicators, and renders a summary with per-task details at the end.

use std::{
    cell::RefCell, io::Write, process::ExitStatus as StdExitStatus, rc::Rc, sync::Arc,
    time::Duration,
};

use bstr::BString;
use owo_colors::Style;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_plan::{ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind};

use super::{
    CACHE_MISS_STYLE, COMMAND_STYLE, ColorizeExt, ExitStatus, GraphExecutionReporter,
    GraphExecutionReporterBuilder, LeafExecutionPath, LeafExecutionReporter, StdinSuggestion,
    format_command_display, write_cache_hit_message, write_command_with_cache_status,
    write_error_message,
};
use crate::session::{
    cache::format_cache_status_summary,
    event::{CacheStatus, CacheUpdateStatus, OutputKind, exit_status_to_code},
};

/// Information tracked for each leaf execution, used in the final summary.
#[derive(Debug)]
struct ExecutionInfo {
    /// Display info for this execution. `None` for displayless executions
    /// (e.g., synthetics reached via nested expansion).
    display: Option<ExecutionItemDisplay>,
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
struct SharedReporterState<W: Write> {
    writer: W,
    executions: Vec<ExecutionInfo>,
    stats: ExecutionStats,
    /// Total number of spawned leaf executions in the graph (including nested `Expanded`
    /// subgraphs). Computed once at build time and used to determine the stdin suggestion:
    /// inherited stdin is only suggested when there is exactly one spawn leaf.
    spawn_leaf_count: usize,
}

/// Count the total number of spawned leaf executions in an execution graph,
/// recursing into nested `Expanded` subgraphs.
///
/// In-process executions are not counted because they don't spawn child processes
/// and thus don't need stdin.
pub(super) fn count_spawn_leaves(graph: &ExecutionGraph) -> usize {
    graph
        .node_weights()
        .flat_map(|task| task.items.iter())
        .map(|item| match &item.kind {
            ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(_)) => 1,
            ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(_)) => 0,
            ExecutionItemKind::Expanded(nested_graph) => count_spawn_leaves(nested_graph),
        })
        .sum()
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
pub struct LabeledReporterBuilder<W: Write> {
    writer: W,
    workspace_path: Arc<AbsolutePath>,
}

impl<W: Write> LabeledReporterBuilder<W> {
    /// Create a new labeled reporter builder.
    ///
    /// - `writer`: The output stream (typically `std::io::stdout()`).
    /// - `workspace_path`: The workspace root, used to compute relative cwds in display.
    pub const fn new(writer: W, workspace_path: Arc<AbsolutePath>) -> Self {
        Self { writer, workspace_path }
    }
}

impl<W: Write + 'static> GraphExecutionReporterBuilder for LabeledReporterBuilder<W> {
    fn build(self: Box<Self>, graph: &Arc<ExecutionGraph>) -> Box<dyn GraphExecutionReporter> {
        let spawn_leaf_count = count_spawn_leaves(graph);
        Box::new(LabeledGraphReporter {
            shared: Rc::new(RefCell::new(SharedReporterState {
                writer: self.writer,
                executions: Vec::new(),
                stats: ExecutionStats::default(),
                spawn_leaf_count,
            })),
            graph: Arc::clone(graph),
            workspace_path: self.workspace_path,
        })
    }
}

/// Graph-level reporter that tracks multiple leaf executions and prints a summary.
///
/// Creates [`LabeledLeafReporter`] instances for each leaf execution. The leaf reporters
/// share mutable state with this reporter via `Rc<RefCell<SharedReporterState>>`.
pub struct LabeledGraphReporter<W: Write> {
    shared: Rc<RefCell<SharedReporterState<W>>>,
    graph: Arc<ExecutionGraph>,
    workspace_path: Arc<AbsolutePath>,
}

impl<W: Write + 'static> GraphExecutionReporter for LabeledGraphReporter<W> {
    fn new_leaf_execution(&mut self, path: &LeafExecutionPath) -> Box<dyn LeafExecutionReporter> {
        // Look up display info from the graph using the path
        let display = path.resolve_display(&self.graph).cloned();
        Box::new(LabeledLeafReporter {
            shared: Rc::clone(&self.shared),
            display,
            workspace_path: Arc::clone(&self.workspace_path),
            started: false,
            is_cache_hit: false,
        })
    }

    fn finish(self: Box<Self>) -> Result<(), ExitStatus> {
        let mut shared = self.shared.borrow_mut();

        // Print summary.
        // Special case: single execution without display info (e.g., synthetic via nested expansion)
        // → skip summary since there's nothing meaningful to show.
        let is_single_displayless =
            shared.executions.len() == 1 && shared.executions[0].display.is_none();
        if !is_single_displayless {
            // Destructure to get simultaneous mutable access to writer and immutable
            // access to executions/stats, satisfying the borrow checker.
            let SharedReporterState { ref mut writer, ref executions, ref stats, .. } = *shared;
            print_summary(writer, executions, stats, &self.workspace_path);
        }

        // Determine exit code based on failed tasks and infrastructure errors:
        // - Infrastructure errors (cache lookup, spawn failure) have error_message set
        //   but no meaningful exit_status.
        // - Process failures have a non-zero exit_status.
        //
        // Rules:
        // 1. No failures at all → Ok(())
        // 2. Exactly one process failure, no infra errors → use that task's exit code
        // 3. Any infra errors, or multiple failures → Err(1)
        let has_infra_errors = shared.executions.iter().any(|exec| exec.error_message.is_some());

        let failed_exit_codes: Vec<i32> = shared
            .executions
            .iter()
            .filter_map(|exec| exec.exit_status.as_ref())
            .filter(|status| !status.success())
            .map(|status| exit_status_to_code(*status))
            .collect();

        match (has_infra_errors, failed_exit_codes.as_slice()) {
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
        }
    }
}

/// Leaf-level reporter created by [`LabeledGraphReporter::new_leaf_execution`].
///
/// Writes output in real-time to the shared writer and updates shared stats/errors
/// via `Rc<RefCell<SharedReporterState>>`.
struct LabeledLeafReporter<W: Write> {
    shared: Rc<RefCell<SharedReporterState<W>>>,
    /// Display info for this execution, looked up from the graph via the path.
    display: Option<ExecutionItemDisplay>,
    workspace_path: Arc<AbsolutePath>,
    /// Whether `start()` has been called. Used to determine if stats should be updated
    /// in `finish()` and whether to push an `ExecutionInfo` entry.
    started: bool,
    /// Whether the current execution is a cache hit, set by `start()`.
    is_cache_hit: bool,
}

impl<W: Write> LeafExecutionReporter for LabeledLeafReporter<W> {
    fn stdin_suggestion(&self) -> StdinSuggestion {
        // Only suggest inherited stdin when the graph has exactly one spawned leaf
        // execution. With multiple spawned tasks, stdin should not be shared — each
        // task gets /dev/null to avoid contention.
        let shared = self.shared.borrow();
        if shared.spawn_leaf_count == 1 {
            StdinSuggestion::Inherited
        } else {
            StdinSuggestion::Null
        }
    }

    fn start(&mut self, cache_status: CacheStatus) {
        self.started = true;
        self.is_cache_hit = matches!(cache_status, CacheStatus::Hit { .. });

        let mut shared = self.shared.borrow_mut();

        // Update statistics based on cache status
        match &cache_status {
            CacheStatus::Hit { .. } => shared.stats.cache_hits += 1,
            CacheStatus::Miss(_) => shared.stats.cache_misses += 1,
            CacheStatus::Disabled(_) => shared.stats.cache_disabled += 1,
        }

        // Print command line with cache status (if display info is available)
        if let Some(ref display) = self.display {
            write_command_with_cache_status(
                &mut shared.writer,
                display,
                &self.workspace_path,
                &cache_status,
            );
        }

        // Store execution info for the summary
        shared.executions.push(ExecutionInfo {
            display: self.display.clone(),
            cache_status,
            exit_status: None,
            error_message: None,
        });
    }

    fn output(&mut self, _kind: OutputKind, content: BString) {
        let mut shared = self.shared.borrow_mut();
        let _ = shared.writer.write_all(&content);
        let _ = shared.writer.flush();
    }

    fn finish(
        self: Box<Self>,
        status: Option<StdExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<Str>,
    ) {
        let mut shared = self.shared.borrow_mut();

        // Handle errors
        if let Some(ref message) = error {
            write_error_message(&mut shared.writer, message);

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
        if error.is_none() && status.is_some_and(|s| !s.success()) {
            shared.stats.failed += 1;
        }

        // Update execution info with exit status (if start() was called and an entry exists)
        if self.started
            && let Some(exec) = shared.executions.last_mut()
        {
            exec.exit_status = status;
        }

        // For executions without display info (synthetics via nested expansion) that are
        // cache hits, print the cache hit message
        if self.started && self.display.is_none() && self.is_cache_hit {
            write_cache_hit_message(&mut shared.writer);
        }

        // Add a trailing newline after each task's output for readability.
        // Skip if start() was never called (e.g. cache lookup failure) — there's
        // no task output to separate.
        if self.started {
            let _ = writeln!(shared.writer);
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Summary printing
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Print the full execution summary with statistics, performance, and per-task details.
///
/// Called by [`LabeledGraphReporter::finish`] after all tasks have executed.
/// Infrastructure errors and task failures are included in the summary.
#[expect(
    clippy::too_many_lines,
    reason = "summary formatting is inherently verbose with many write calls"
)]
fn print_summary(
    writer: &mut impl Write,
    executions: &[ExecutionInfo],
    stats: &ExecutionStats,
    workspace_path: &AbsolutePath,
) {
    let total = executions.len();
    let cache_hits = stats.cache_hits;
    let cache_misses = stats.cache_misses;
    let cache_disabled = stats.cache_disabled;
    let failed = stats.failed;

    // Print summary header with decorative line
    // Note: leaf finish already adds a trailing newline after each task's output
    // Add an extra blank line before the summary for visual separation
    let _ = writeln!(writer);
    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(
        writer,
        "{}",
        "    Vite+ Task Runner • Execution Summary".style(Style::new().bold().bright_white())
    );
    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(writer);

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
    // Note: trailing space after "cache misses" is intentional for consistent formatting
    let _ = write!(
        writer,
        "{}  {} {} {} ",
        "Statistics:".style(Style::new().bold()),
        vite_str::format!(" {total} tasks").style(Style::new().bright_white()),
        vite_str::format!("• {cache_hits} cache hits").style(Style::new().green()),
        vite_str::format!("• {cache_misses} cache misses").style(CACHE_MISS_STYLE),
    );
    if !cache_disabled_str.is_empty() {
        let _ = write!(writer, "{cache_disabled_str} ");
    }
    if !failed_str.is_empty() {
        let _ = write!(writer, "{failed_str} ");
    }
    let _ = writeln!(writer);

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
        writer,
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
        let _ = write!(
            writer,
            ", {:.2?} saved in total",
            total_saved.style(Style::new().green().bold())
        );
    }
    let _ = writeln!(writer);
    let _ = writeln!(writer);

    // Detailed task results
    let _ = writeln!(writer, "{}", "Task Details:".style(Style::new().bold()));
    let _ = writeln!(
        writer,
        "{}",
        "────────────────────────────────────────────────".style(Style::new().bright_black())
    );

    for (idx, exec) in executions.iter().enumerate() {
        // Skip executions without display info (they have nothing to show in the summary)
        let Some(ref display) = exec.display else {
            continue;
        };

        let task_display = &display.task_display;

        // Task name and index
        let _ = write!(
            writer,
            "  {} {}",
            vite_str::format!("[{}]", idx + 1).style(Style::new().bright_black()),
            task_display.to_string().style(Style::new().bright_white().bold())
        );

        // Command with cwd prefix
        let command_display = format_command_display(display, workspace_path);
        let _ = write!(writer, ": {}", command_display.style(COMMAND_STYLE));

        // Execution result icon
        // None means success (cache hit or in-process), Some checks actual status
        match &exec.exit_status {
            None => {
                let _ = write!(writer, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) if exit_status.success() => {
                let _ = write!(writer, " {}", "✓".style(Style::new().green().bold()));
            }
            Some(exit_status) => {
                let code = exit_status_to_code(*exit_status);
                let _ = write!(
                    writer,
                    " {} {}",
                    "✗".style(Style::new().red().bold()),
                    vite_str::format!("(exit code: {code})").style(Style::new().red())
                );
            }
        }
        let _ = writeln!(writer);

        // Cache status details — use display module for plain text, apply styling here
        let cache_summary = format_cache_status_summary(&exec.cache_status);
        let styled_summary = match &exec.cache_status {
            CacheStatus::Hit { .. } => cache_summary.style(Style::new().green()),
            CacheStatus::Miss(_) => cache_summary.style(CACHE_MISS_STYLE),
            CacheStatus::Disabled(_) => cache_summary.style(Style::new().bright_black()),
        };
        let _ = writeln!(writer, "      {styled_summary}");

        // Error message if present
        if let Some(ref error_msg) = exec.error_message {
            let _ = writeln!(
                writer,
                "      {} {}",
                "✗ Error:".style(Style::new().red().bold()),
                error_msg.style(Style::new().red())
            );
        }

        // Add spacing between tasks except for the last one
        if idx < executions.len() - 1 {
            let _ = writeln!(
                writer,
                "  {}",
                "·······················································"
                    .style(Style::new().bright_black())
            );
        }
    }

    let _ = writeln!(
        writer,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
}

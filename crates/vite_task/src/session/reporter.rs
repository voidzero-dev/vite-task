//! LabeledReporter event handler for rendering execution events.

use std::{
    collections::HashSet,
    io::Write,
    process::ExitStatus as StdExitStatus,
    sync::{Arc, LazyLock},
    time::Duration,
};

use owo_colors::{Style, Styled};
use vite_path::AbsolutePath;

use super::{
    cache::{format_cache_status_inline, format_cache_status_summary},
    event::{
        CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId, ExecutionItemDisplay,
        exit_status_to_code,
    },
};

/// Wrap of `OwoColorize` that ignores style if `NO_COLOR` is set.
trait ColorizeExt {
    fn style(&self, style: Style) -> Styled<&Self>;
}

impl<T: owo_colors::OwoColorize> ColorizeExt for T {
    fn style(&self, style: Style) -> Styled<&Self> {
        static NO_COLOR: LazyLock<bool> =
            LazyLock::new(|| std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()));
        owo_colors::OwoColorize::style(self, if *NO_COLOR { Style::new() } else { style })
    }
}

/// Exit status code for task execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus(pub u8);

impl ExitStatus {
    pub const FAILURE: Self = Self(1);
    pub const SUCCESS: Self = Self(0);
}

/// Trait for handling execution events and reporting results
pub trait Reporter {
    /// Handle an execution event (start, output, error, finish)
    fn handle_event(&mut self, event: ExecutionEvent);

    /// Called after execution completes (whether successful or not)
    /// Returns Ok(()) on success, or Err(ExitStatus) on failure
    fn post_execution(self: Box<Self>) -> Result<(), ExitStatus>;
}

const COMMAND_STYLE: Style = Style::new().cyan();
const CACHE_MISS_STYLE: Style = Style::new().purple();

/// Information tracked for each execution
#[derive(Debug)]
struct ExecutionInfo {
    display: Option<ExecutionItemDisplay>,
    cache_status: CacheStatus, // Non-optional, determined at Start
    /// Exit status from the process. None means no process was spawned (cache hit or in-process).
    exit_status: Option<StdExitStatus>,
    error_message: Option<String>,
}

/// Statistics for the execution summary
#[derive(Default)]
struct ExecutionStats {
    cache_hits: usize,
    cache_misses: usize,
    cache_disabled: usize,
    failed: usize,
}

/// Event handler that renders execution events in labeled format.
///
/// # Output Modes
///
/// The reporter has different output modes based on configuration and execution context:
///
/// ## Normal Mode (default)
/// - Prints command lines with cache status indicators during execution
/// - Shows full summary with Statistics and Task Details at the end
///
/// ## Silent Cache Hit Mode (`silent_if_cache_hit = true`)
/// - Suppresses command lines and output for cache hit executions
/// - Useful for faster, cleaner output when many tasks are cached
///
/// ## Hidden Summary Mode (`hide_summary = true`)
/// - Skips printing the execution summary entirely
/// - Useful for programmatic usage or when summary is not needed
///
/// ## Simplified Summary for Single Tasks
/// - When a single task is executed:
///   - Skips full summary (no Statistics/Task Details sections)
///   - Shows only cache status (except for "NotFound" which is hidden for clean first-run output)
///   - Results in clean output showing just the command's stdout/stderr
pub struct LabeledReporter<W: Write> {
    writer: W,
    workspace_path: Arc<AbsolutePath>,
    executions: Vec<ExecutionInfo>,
    stats: ExecutionStats,
    first_error: Option<String>,

    /// When true, suppresses command line and output for cache hit executions
    silent_if_cache_hit: bool,

    /// When true, skips printing the execution summary at the end
    hide_summary: bool,

    /// Tracks which executions are cache hits (for silent_if_cache_hit mode)
    cache_hit_executions: HashSet<ExecutionId>,
}

impl<W: Write> LabeledReporter<W> {
    pub fn new(writer: W, workspace_path: Arc<AbsolutePath>) -> Self {
        Self {
            writer,
            workspace_path,
            executions: Vec::new(),
            stats: ExecutionStats::default(),
            first_error: None,
            silent_if_cache_hit: false,
            hide_summary: false,
            cache_hit_executions: HashSet::new(),
        }
    }

    /// Set the silent_if_cache_hit option
    pub fn set_silent_if_cache_hit(&mut self, silent_if_cache_hit: bool) {
        self.silent_if_cache_hit = silent_if_cache_hit;
    }

    /// Set the hide_summary option
    pub fn set_hide_summary(&mut self, hide_summary: bool) {
        self.hide_summary = hide_summary;
    }

    fn handle_start(
        &mut self,
        execution_id: ExecutionId,
        display: Option<ExecutionItemDisplay>,
        cache_status: CacheStatus,
    ) {
        // Update statistics immediately based on cache status
        match &cache_status {
            CacheStatus::Hit { .. } => {
                self.stats.cache_hits += 1;
                // Track cache hit executions for silent mode
                if self.silent_if_cache_hit {
                    self.cache_hit_executions.insert(execution_id);
                }
            }
            CacheStatus::Miss(_) => self.stats.cache_misses += 1,
            CacheStatus::Disabled(_) => self.stats.cache_disabled += 1,
        }

        // Handle None display case - direct synthetic execution (e.g., via plan_exec)
        // Don't print cache status here - will be printed at finish for cache hits only
        let Some(display) = display else {
            self.executions.push(ExecutionInfo {
                display: None,
                cache_status,
                exit_status: None,
                error_message: None,
            });
            return;
        };

        // Compute cwd relative to workspace root
        let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(&self.workspace_path) {
            rel.as_str().to_string()
        } else {
            String::new()
        };

        let cwd_str =
            if cwd_relative.is_empty() { String::new() } else { format!("~/{cwd_relative}") };
        let command_str = format!("{cwd_str}$ {}", display.command);

        // Skip printing if silent_if_cache_hit is enabled and this is a cache hit
        let should_print =
            !self.silent_if_cache_hit || !matches!(cache_status, CacheStatus::Hit { .. });

        if should_print {
            // Print command with optional inline cache status
            // Use display module for plain text, apply styling here
            if let Some(inline_status) = format_cache_status_inline(&cache_status) {
                // Apply styling based on cache status type
                let styled_status = match &cache_status {
                    CacheStatus::Hit { .. } => inline_status.style(Style::new().green().dimmed()),
                    CacheStatus::Miss(_) => inline_status.style(CACHE_MISS_STYLE.dimmed()),
                    CacheStatus::Disabled(_) => inline_status.style(Style::new().bright_black()),
                };
                let _ =
                    writeln!(self.writer, "{} {}", command_str.style(COMMAND_STYLE), styled_status);
            } else {
                let _ = writeln!(self.writer, "{}", command_str.style(COMMAND_STYLE));
            }
        }

        // Store execution info for summary
        self.executions.push(ExecutionInfo {
            display: Some(display),
            cache_status,
            exit_status: None,
            error_message: None,
        });
    }

    fn handle_error(&mut self, _execution_id: ExecutionId, message: String) {
        // Display error inline (in red, with error icon)
        let _ = writeln!(
            self.writer,
            "{} {}",
            "✗".style(Style::new().red().bold()),
            message.style(Style::new().red())
        );

        // Track first error
        if self.first_error.is_none() {
            self.first_error = Some(message.clone());
        }

        // Track error for summary
        if let Some(exec) = self.executions.last_mut() {
            exec.error_message = Some(message);
        }

        self.stats.failed += 1;
    }

    fn handle_finish(&mut self, execution_id: ExecutionId, status: Option<StdExitStatus>) {
        // Update failure statistics
        // None means success (cache hit or in-process), Some checks the actual exit status
        if status.is_some_and(|s| !s.success()) {
            self.stats.failed += 1;
        }

        // Update execution info exit status
        if let Some(exec) = self.executions.last_mut() {
            exec.exit_status = status;
        }

        // For direct synthetic execution with cache hit, print message at the bottom
        if let Some(exec) = self.executions.last() {
            if exec.display.is_none() && matches!(exec.cache_status, CacheStatus::Hit { .. }) {
                let should_print =
                    !self.silent_if_cache_hit || !self.cache_hit_executions.contains(&execution_id);
                if should_print {
                    let _ = writeln!(
                        self.writer,
                        "{}",
                        "✓ cache hit, logs replayed".style(Style::new().green().dimmed())
                    );
                }
            }
        }

        // Add a line break after each task's output for better readability
        // Skip if silent_if_cache_hit is enabled and this execution is a cache hit
        if !self.silent_if_cache_hit || !self.cache_hit_executions.contains(&execution_id) {
            let _ = writeln!(self.writer);
        }
    }

    /// Print execution summary after all events
    pub fn print_summary(&mut self) {
        let total = self.executions.len();
        let cache_hits = self.stats.cache_hits;
        let cache_misses = self.stats.cache_misses;
        let cache_disabled = self.stats.cache_disabled;
        let failed = self.stats.failed;

        // Print summary header with decorative line
        // Note: handle_finish already adds a trailing newline after each task's output
        // Add an extra blank line before the summary for visual separation
        let _ = writeln!(self.writer);
        let _ = writeln!(
            self.writer,
            "{}",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
        );
        let _ = writeln!(
            self.writer,
            "{}",
            "    Vite+ Task Runner • Execution Summary".style(Style::new().bold().bright_white())
        );
        let _ = writeln!(
            self.writer,
            "{}",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
        );
        let _ = writeln!(self.writer);

        // Print statistics
        let cache_disabled_str = if cache_disabled > 0 {
            format!("• {cache_disabled} cache disabled")
                .style(Style::new().bright_black())
                .to_string()
        } else {
            String::new()
        };

        let failed_str = if failed > 0 {
            format!("• {failed} failed").style(Style::new().red()).to_string()
        } else {
            String::new()
        };

        // Build statistics line, only including non-empty parts
        // Note: trailing space after "cache misses" is intentional for consistent formatting
        let _ = write!(
            self.writer,
            "{}  {} {} {} ",
            "Statistics:".style(Style::new().bold()),
            format!(" {total} tasks").style(Style::new().bright_white()),
            format!("• {cache_hits} cache hits").style(Style::new().green()),
            format!("• {cache_misses} cache misses").style(CACHE_MISS_STYLE),
        );
        if !cache_disabled_str.is_empty() {
            let _ = write!(self.writer, "{} ", cache_disabled_str);
        }
        if !failed_str.is_empty() {
            let _ = write!(self.writer, "{} ", failed_str);
        }
        let _ = writeln!(self.writer);

        // Calculate cache hit rate
        let cache_rate = if total > 0 {
            (f64::from(cache_hits as u32) / total as f64 * 100.0) as u32
        } else {
            0
        };

        // Calculate total time saved
        let total_saved: Duration = self
            .executions
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
            self.writer,
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
                self.writer,
                ", {:.2?} saved in total",
                total_saved.style(Style::new().green().bold())
            );
        }
        let _ = writeln!(self.writer);
        let _ = writeln!(self.writer);

        // Detailed task results
        let _ = writeln!(self.writer, "{}", "Task Details:".style(Style::new().bold()));
        let _ = writeln!(
            self.writer,
            "{}",
            "────────────────────────────────────────────────".style(Style::new().bright_black())
        );

        for (idx, exec) in self.executions.iter().enumerate() {
            // Skip if no display info
            let Some(ref display) = exec.display else {
                continue;
            };

            let task_display = &display.task_display;

            // Task name and index
            let _ = write!(
                self.writer,
                "  {} {}",
                format!("[{}]", idx + 1).style(Style::new().bright_black()),
                task_display.to_string().style(Style::new().bright_white().bold())
            );

            // Command with cwd prefix
            let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(&self.workspace_path)
            {
                rel.as_str().to_string()
            } else {
                String::new()
            };
            let cwd_str =
                if cwd_relative.is_empty() { String::new() } else { format!("~/{cwd_relative}") };
            let command_display = format!("{cwd_str}$ {}", display.command);
            let _ = write!(self.writer, ": {}", command_display.style(COMMAND_STYLE));

            // Execution result icon
            // None means success (cache hit or in-process), Some checks actual status
            match &exec.exit_status {
                None => {
                    let _ = write!(self.writer, " {}", "✓".style(Style::new().green().bold()));
                }
                Some(status) if status.success() => {
                    let _ = write!(self.writer, " {}", "✓".style(Style::new().green().bold()));
                }
                Some(status) => {
                    let code = exit_status_to_code(status);
                    let _ = write!(
                        self.writer,
                        " {} {}",
                        "✗".style(Style::new().red().bold()),
                        format!("(exit code: {code})").style(Style::new().red())
                    );
                }
            }
            let _ = writeln!(self.writer);

            // Cache status details - use display module for plain text, apply styling here
            let cache_summary = format_cache_status_summary(&exec.cache_status);
            let styled_summary = match &exec.cache_status {
                CacheStatus::Hit { .. } => cache_summary.style(Style::new().green()),
                CacheStatus::Miss(_) => cache_summary.style(CACHE_MISS_STYLE),
                CacheStatus::Disabled(_) => cache_summary.style(Style::new().bright_black()),
            };
            let _ = writeln!(self.writer, "      {}", styled_summary);

            // Error message if present
            if let Some(ref error_msg) = exec.error_message {
                let _ = writeln!(
                    self.writer,
                    "      {} {}",
                    "✗ Error:".style(Style::new().red().bold()),
                    error_msg.style(Style::new().red())
                );
            }

            // Add spacing between tasks except for the last one
            if idx < self.executions.len() - 1 {
                let _ = writeln!(
                    self.writer,
                    "  {}",
                    "·······················································"
                        .style(Style::new().bright_black())
                );
            }
        }

        let _ = writeln!(
            self.writer,
            "{}",
            "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
        );
    }

    /// Print simplified cache status for single built-in commands
    ///
    /// Note: Inline cache status is now printed at Start event in handle_start(),
    /// so this function is a no-op to avoid duplicate output.
    fn print_simple_cache_status(&mut self) {
        // Inline cache status already printed at Start event - nothing to do here
    }
}

impl<W: Write> Reporter for LabeledReporter<W> {
    fn handle_event(&mut self, event: ExecutionEvent) {
        match event.kind {
            ExecutionEventKind::Start { display, cache_status } => {
                self.handle_start(event.execution_id, display, cache_status);
            }
            ExecutionEventKind::Output { content, .. } => {
                // Skip output if silent_if_cache_hit is enabled and this execution is a cache hit
                if self.silent_if_cache_hit
                    && self.cache_hit_executions.contains(&event.execution_id)
                {
                    return;
                }
                let _ = self.writer.write_all(&content);
                let _ = self.writer.flush();
            }
            ExecutionEventKind::Error { message } => {
                self.handle_error(event.execution_id, message);
            }
            ExecutionEventKind::Finish { status, cache_update_status: _ } => {
                self.handle_finish(event.execution_id, status);
            }
        }
    }

    fn post_execution(mut self: Box<Self>) -> Result<(), ExitStatus> {
        // Check if execution was aborted due to error
        if let Some(error_msg) = &self.first_error {
            // Print separator
            let _ = writeln!(self.writer);
            let _ = writeln!(
                self.writer,
                "{}",
                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
                    .style(Style::new().bright_black())
            );

            // Print error abort message
            let _ = writeln!(
                self.writer,
                "{} {}",
                "Execution aborted due to error:".style(Style::new().red().bold()),
                error_msg.style(Style::new().red())
            );

            let _ = writeln!(
                self.writer,
                "{}",
                "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
                    .style(Style::new().bright_black())
            );

            return Err(ExitStatus::FAILURE);
        }

        // No errors - print summary if not hidden
        if !self.hide_summary {
            // Special case: single built-in command (no display info)
            if self.executions.len() == 1 && self.executions[0].display.is_none() {
                self.print_simple_cache_status();
            } else {
                self.print_summary();
            }
        }

        // Determine exit code based on failed tasks:
        // 1. All tasks succeed → return Ok(())
        // 2. Exactly one task failed → return Err with that task's exit code
        // 3. More than one task failed → return Err(1)
        // Note: None means success (cache hit or in-process)
        let failed_exit_codes: Vec<i32> = self
            .executions
            .iter()
            .filter_map(|exec| exec.exit_status.as_ref())
            .filter(|status| !status.success())
            .map(exit_status_to_code)
            .collect();

        match failed_exit_codes.as_slice() {
            [] => Ok(()),
            [code] => {
                // Return the single failed task's exit code (clamped to u8 range)
                Err(ExitStatus((*code).clamp(1, 255) as u8))
            }
            _ => Err(ExitStatus::FAILURE),
        }
    }
}

//! LabeledReporter event handler for rendering execution events.

use std::{
    io::Write,
    sync::{Arc, LazyLock},
    time::Duration,
};

use owo_colors::{Style, Styled};
use vite_path::AbsolutePath;

use super::{
    cache::{format_cache_status_inline, format_cache_status_summary},
    event::{CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId, ExecutionItemDisplay},
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

/// Trait for handling execution events and reporting results
pub trait Reporter {
    /// Handle an execution event (start, output, error, finish)
    fn handle_event(&mut self, event: ExecutionEvent);

    /// Called after execution completes (whether successful or not)
    /// Returns Err if execution failed due to errors
    fn post_execution(self: Box<Self>) -> anyhow::Result<()>;
}

const COMMAND_STYLE: Style = Style::new().cyan();
const CACHE_MISS_STYLE: Style = Style::new().purple();

/// Information tracked for each execution
#[derive(Debug)]
struct ExecutionInfo {
    display: Option<ExecutionItemDisplay>,
    cache_status: CacheStatus, // Non-optional, determined at Start
    exit_status: Option<i32>,
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
pub struct LabeledReporter<W: Write> {
    writer: W,
    workspace_path: Arc<AbsolutePath>,
    executions: Vec<ExecutionInfo>,
    stats: ExecutionStats,
    first_error: Option<String>,
}

impl<W: Write> LabeledReporter<W> {
    pub fn new(writer: W, workspace_path: Arc<AbsolutePath>) -> Self {
        Self {
            writer,
            workspace_path,
            executions: Vec::new(),
            stats: ExecutionStats::default(),
            first_error: None,
        }
    }

    fn handle_start(&mut self, display: Option<ExecutionItemDisplay>, cache_status: CacheStatus) {
        // Update statistics immediately based on cache status
        match &cache_status {
            CacheStatus::Hit { .. } => self.stats.cache_hits += 1,
            CacheStatus::Miss(_) => self.stats.cache_misses += 1,
            CacheStatus::Disabled(_) => self.stats.cache_disabled += 1,
        }

        // Handle None display case - just store minimal info
        // This occurs for top-level execution (no parent task)
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
            if cwd_relative.is_empty() { String::new() } else { format!("{cwd_relative}/") };
        let command_str = format!("{cwd_str}$ {}", display.command);

        // Print command with optional inline cache status
        // Use display module for plain text, apply styling here
        if let Some(inline_status) = format_cache_status_inline(&cache_status) {
            // Apply styling based on cache status type
            let styled_status = match &cache_status {
                CacheStatus::Hit { .. } => inline_status.style(Style::new().green().dimmed()),
                CacheStatus::Miss(_) => inline_status.style(CACHE_MISS_STYLE.dimmed()),
                CacheStatus::Disabled(_) => inline_status.style(Style::new().bright_black()),
            };
            let _ = writeln!(self.writer, "{} {}", command_str.style(COMMAND_STYLE), styled_status);
        } else {
            let _ = writeln!(self.writer, "{}", command_str.style(COMMAND_STYLE));
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

    fn handle_finish(&mut self, _execution_id: ExecutionId, status: Option<i32>) {
        // Update failure statistics
        if let Some(s) = status {
            if s != 0 {
                self.stats.failed += 1;
            }
        }

        // Update execution info exit status
        if let Some(exec) = self.executions.last_mut() {
            exec.exit_status = status;
        }
    }

    /// Print execution summary after all events
    pub fn print_summary(&mut self) {
        let total = self.executions.len();
        let cache_hits = self.stats.cache_hits;
        let cache_misses = self.stats.cache_misses;
        let failed = self.stats.failed;

        // Print summary header with decorative line
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
        let failed_str = if failed > 0 {
            format!("• {failed} failed").style(Style::new().red()).to_string()
        } else {
            String::new()
        };

        let _ = writeln!(
            self.writer,
            "{}  {} {} {} {}",
            "Statistics:".style(Style::new().bold()),
            format!(" {total} tasks").style(Style::new().bright_white()),
            format!("• {cache_hits} cache hits").style(Style::new().green()),
            format!("• {cache_misses} cache misses").style(CACHE_MISS_STYLE),
            failed_str
        );

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

            let task_name = &display.task_display.task_name;

            // Task name and index
            let _ = write!(
                self.writer,
                "  {} {}",
                format!("[{}]", idx + 1).style(Style::new().bright_black()),
                task_name.style(Style::new().bright_white().bold())
            );

            // Command
            let _ = write!(self.writer, ": {}", display.command.style(COMMAND_STYLE));

            // Execution result icon
            match exec.exit_status {
                Some(0) => {
                    let _ = write!(self.writer, " {}", "✓".style(Style::new().green().bold()));
                }
                Some(code) => {
                    let _ = write!(
                        self.writer,
                        " {} {}",
                        "✗".style(Style::new().red().bold()),
                        format!("(exit code: {code})").style(Style::new().red())
                    );
                }
                None => {
                    let _ = write!(self.writer, " {}", "?".style(Style::new().bright_black()));
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
}

impl<W: Write> Reporter for LabeledReporter<W> {
    fn handle_event(&mut self, event: ExecutionEvent) {
        match event.kind {
            ExecutionEventKind::Start { display, cache_status } => {
                self.handle_start(display, cache_status);
            }
            ExecutionEventKind::Output { content, .. } => {
                let _ = self.writer.write_all(&content);
                let _ = self.writer.flush();
            }
            ExecutionEventKind::Error { message } => {
                self.handle_error(event.execution_id, message);
            }
            ExecutionEventKind::Finish { status } => {
                self.handle_finish(event.execution_id, status);
            }
        }
    }

    fn post_execution(mut self: Box<Self>) -> anyhow::Result<()> {
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

            return Err(anyhow::anyhow!("Execution aborted: {}", error_msg));
        }

        // No errors - print summary
        self.print_summary();
        Ok(())
    }
}

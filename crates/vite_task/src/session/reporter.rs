//! LabeledReporter event handler for rendering execution events.

use std::{
    io::Write,
    sync::{Arc, LazyLock},
    time::Duration,
};

use owo_colors::{Style, Styled};
use vite_path::AbsolutePath;

use super::{
    cache::{CacheMiss, FingerprintMismatch},
    event::{CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId, ExecutionItemDisplay},
    execute::fingerprint::PostRunFingerprintMismatch,
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

const COMMAND_STYLE: Style = Style::new().cyan();
const CACHE_MISS_STYLE: Style = Style::new().purple();

/// Information tracked for each execution
#[derive(Debug)]
struct ExecutionInfo {
    display: ExecutionItemDisplay,
    cache_status: Option<CacheStatus>,
    exit_status: Option<i32>,
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
}

impl<W: Write> LabeledReporter<W> {
    pub fn new(writer: W, workspace_path: Arc<AbsolutePath>) -> Self {
        Self { writer, workspace_path, executions: Vec::new(), stats: ExecutionStats::default() }
    }

    /// Handle an execution event - called by Session::execute()
    pub fn handle_event(&mut self, event: ExecutionEvent, cache_miss: Option<&CacheMiss>) {
        match event.kind {
            ExecutionEventKind::Start(display) => {
                self.handle_start(display, cache_miss);
            }
            ExecutionEventKind::Output { content, .. } => {
                // Stream output directly to writer
                let _ = self.writer.write_all(&content);
                let _ = self.writer.flush();
            }
            ExecutionEventKind::Finish { status, cache_status } => {
                self.handle_finish(event.execution_id, status, cache_status);
            }
        }
    }

    fn handle_start(&mut self, display: ExecutionItemDisplay, cache_miss: Option<&CacheMiss>) {
        // Compute cwd relative to workspace root
        let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(&self.workspace_path) {
            rel.as_str().to_string()
        } else {
            String::new()
        };

        let cwd_str =
            if cwd_relative.is_empty() { String::new() } else { format!("{cwd_relative}/") };
        let command_str = format!("{cwd_str}$ {}", display.command);

        // Print command with cache status
        match cache_miss {
            None => {
                // Cache miss: not found - just print command
                let _ = writeln!(self.writer, "{}", command_str.style(COMMAND_STYLE));
            }
            Some(CacheMiss::NotFound) => {
                // Cache miss: not found - just print command
                let _ = writeln!(self.writer, "{}", command_str.style(COMMAND_STYLE));
            }
            Some(CacheMiss::FingerprintMismatch(mismatch)) => {
                // Cache miss: fingerprint mismatch
                let reason = match mismatch {
                    FingerprintMismatch::SpawnFingerprintMismatch(_) => {
                        "command configuration changed".to_string()
                    }
                    FingerprintMismatch::PostRunFingerprintMismatch(
                        PostRunFingerprintMismatch::InputContentChanged { path },
                    ) => {
                        format!("content of input '{path}' changed")
                    }
                };
                let _ = write!(self.writer, "{} ", command_str.style(COMMAND_STYLE));
                let _ = writeln!(
                    self.writer,
                    "{}",
                    format_args!("(✗ cache miss: {reason}, executing)")
                        .style(CACHE_MISS_STYLE.dimmed())
                );
            }
        }

        // Store execution info for summary
        self.executions.push(ExecutionInfo { display, cache_status: None, exit_status: None });
    }

    fn handle_finish(
        &mut self,
        _execution_id: ExecutionId,
        status: Option<i32>,
        cache_status: CacheStatus,
    ) {
        // Update statistics
        match &cache_status {
            CacheStatus::Hit { .. } => self.stats.cache_hits += 1,
            CacheStatus::Miss => self.stats.cache_misses += 1,
            CacheStatus::Disabled(_) => self.stats.cache_disabled += 1,
        }

        if let Some(s) = status {
            if s != 0 {
                self.stats.failed += 1;
            }
        }

        // Update execution info if we have it
        if let Some(exec) = self.executions.last_mut() {
            exec.cache_status = Some(cache_status);
            exec.exit_status = status;
        }
    }

    /// Handle a cache hit event - prints replay message and outputs
    pub fn handle_cache_hit(&mut self, display: ExecutionItemDisplay, duration: Duration) {
        // Compute cwd relative to workspace root
        let cwd_relative = if let Ok(Some(rel)) = display.cwd.strip_prefix(&self.workspace_path) {
            rel.as_str().to_string()
        } else {
            String::new()
        };

        let cwd_str =
            if cwd_relative.is_empty() { String::new() } else { format!("{cwd_relative}/") };
        let command_str = format!("{cwd_str}$ {}", display.command);

        let _ = write!(self.writer, "{} ", command_str.style(COMMAND_STYLE));
        let _ = writeln!(
            self.writer,
            "{}",
            "(✓ cache hit, replaying)".style(Style::new().green().dimmed())
        );

        // Store execution info
        self.executions.push(ExecutionInfo {
            display,
            cache_status: Some(CacheStatus::Hit { replayed_duration: duration }),
            exit_status: Some(0),
        });

        self.stats.cache_hits += 1;
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
                if let Some(CacheStatus::Hit { replayed_duration }) = &exec.cache_status {
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
            let task_name = &exec.display.task_display.task_name;

            // Task name and index
            let _ = write!(
                self.writer,
                "  {} {}",
                format!("[{}]", idx + 1).style(Style::new().bright_black()),
                task_name.style(Style::new().bright_white().bold())
            );

            // Command
            let _ = write!(self.writer, ": {}", exec.display.command.style(COMMAND_STYLE));

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

            // Cache status details
            match &exec.cache_status {
                Some(CacheStatus::Hit { replayed_duration }) => {
                    let _ = writeln!(
                        self.writer,
                        "      {} {}",
                        "→ Cache hit - output replayed".style(Style::new().green()),
                        format!("- {replayed_duration:.2?} saved").style(Style::new().green())
                    );
                }
                Some(CacheStatus::Miss) => {
                    let _ =
                        writeln!(self.writer, "      {}", "→ Cache miss".style(CACHE_MISS_STYLE));
                }
                Some(CacheStatus::Disabled(reason)) => {
                    let _ = writeln!(
                        self.writer,
                        "      {}",
                        format!("→ Cache disabled: {reason:?}").style(Style::new().bright_black())
                    );
                }
                None => {}
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

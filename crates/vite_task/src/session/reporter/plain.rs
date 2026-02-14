//! Plain reporter — a standalone [`LeafExecutionReporter`] for single-leaf execution.
//!
//! Used for synthetic executions (e.g., auto-install) where there is no execution graph
//! and no summary is needed. Owns its writer directly with no shared state.

use std::io::Write;

use bstr::BString;
use vite_str::Str;

use super::{LeafExecutionReporter, StdinSuggestion, write_cache_hit_message, write_error_message};
use crate::session::event::{CacheStatus, CacheUpdateStatus, OutputKind};

/// A self-contained [`LeafExecutionReporter`] for single-leaf executions
/// (e.g., `execute_synthetic`).
///
/// This reporter:
/// - Owns its writer directly (no `Rc<RefCell>` shared state)
/// - Has no display info (synthetic executions have no task display)
/// - Does not track stats or print summaries
/// - Supports `silent_if_cache_hit` to suppress output for cached executions
///
/// The exit status is determined by the caller from the `execute_spawn` return value,
/// not from the reporter.
pub struct PlainReporter<W: Write> {
    writer: W,
    /// When true, suppresses all output (command line, process output, cache hit message)
    /// for executions that are cache hits.
    silent_if_cache_hit: bool,
    /// Whether the current execution is a cache hit, set by `start()`.
    is_cache_hit: bool,
}

impl<W: Write> PlainReporter<W> {
    /// Create a new plain reporter.
    ///
    /// - `writer`: The output stream (typically `std::io::stdout()`).
    /// - `silent_if_cache_hit`: If true, suppress all output when the execution is a cache hit.
    pub const fn new(writer: W, silent_if_cache_hit: bool) -> Self {
        Self { writer, silent_if_cache_hit, is_cache_hit: false }
    }

    /// Returns true if output should be suppressed for this execution.
    const fn is_silent(&self) -> bool {
        self.silent_if_cache_hit && self.is_cache_hit
    }
}

impl<W: Write> LeafExecutionReporter for PlainReporter<W> {
    fn stdin_suggestion(&self) -> StdinSuggestion {
        // PlainReporter is used for single-leaf synthetic executions (e.g., auto-install).
        // Always suggest inherited stdin so the spawned process can be interactive.
        StdinSuggestion::Inherited
    }

    fn start(&mut self, cache_status: CacheStatus) {
        self.is_cache_hit = matches!(cache_status, CacheStatus::Hit { .. });
        // PlainReporter has no display info (synthetic executions),
        // so there's no command line to print at start.
    }

    fn output(&mut self, _kind: OutputKind, content: BString) {
        if self.is_silent() {
            return;
        }
        let _ = self.writer.write_all(&content);
        let _ = self.writer.flush();
    }

    fn finish(
        mut self: Box<Self>,
        _status: Option<std::process::ExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<Str>,
    ) {
        // Handle errors — print inline error message
        if let Some(ref message) = error {
            write_error_message(&mut self.writer, message);
            return;
        }

        // For cache hits, print the "cache hit" message (unless silent)
        if self.is_cache_hit && !self.is_silent() {
            write_cache_hit_message(&mut self.writer);
        }
    }
}

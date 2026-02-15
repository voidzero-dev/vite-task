//! Plain reporter — a standalone [`LeafExecutionReporter`] for single-leaf execution.
//!
//! Used for synthetic executions (e.g., auto-install) where there is no execution graph
//! and no summary is needed. Writes directly to stdout/stderr with no shared state.

use super::{
    LeafExecutionReporter, StdioConfig, StdioSuggestion, write_cache_hit_message,
    write_error_message,
};
use crate::session::event::{CacheStatus, CacheUpdateStatus, ExecutionError};

/// A self-contained [`LeafExecutionReporter`] for single-leaf executions
/// (e.g., `execute_synthetic`).
///
/// This reporter:
/// - Writes display output (errors, cache-hit messages) directly to stdout
/// - Has no display info (synthetic executions have no task display)
/// - Does not track stats or print summaries
/// - Supports `silent_if_cache_hit` to suppress output for cached executions
///
/// The exit status is determined by the caller from the `execute_spawn` return value,
/// not from the reporter.
pub struct PlainReporter {
    /// When true, suppresses all output (command line, process output, cache hit message)
    /// for executions that are cache hits.
    silent_if_cache_hit: bool,
    /// Whether the current execution is a cache hit, set by `start()`.
    is_cache_hit: bool,
}

impl PlainReporter {
    /// Create a new plain reporter.
    ///
    /// - `silent_if_cache_hit`: If true, suppress all output when the execution is a cache hit.
    pub const fn new(silent_if_cache_hit: bool) -> Self {
        Self { silent_if_cache_hit, is_cache_hit: false }
    }

    /// Returns true if output should be suppressed for this execution.
    const fn is_silent(&self) -> bool {
        self.silent_if_cache_hit && self.is_cache_hit
    }
}

impl LeafExecutionReporter for PlainReporter {
    fn start(&mut self, cache_status: CacheStatus) -> StdioConfig {
        self.is_cache_hit = matches!(cache_status, CacheStatus::Hit { .. });
        // PlainReporter is used for single-leaf synthetic executions (e.g., auto-install).
        // Always suggest inherited stdio so the spawned process can be interactive.
        // PlainReporter has no display info (synthetic executions),
        // so there's no command line to print at start.
        //
        // When silent_if_cache_hit is enabled and we have a cache hit, return
        // sink writers that discard output — the cache replay in execute_spawn
        // writes directly to these writers, so this is the reporter's only way
        // to suppress replayed output.
        if self.silent_if_cache_hit && self.is_cache_hit {
            StdioConfig {
                suggestion: StdioSuggestion::Inherited,
                stdout_writer: Box::new(tokio::io::sink()),
                stderr_writer: Box::new(tokio::io::sink()),
            }
        } else {
            StdioConfig {
                suggestion: StdioSuggestion::Inherited,
                stdout_writer: Box::new(tokio::io::stdout()),
                stderr_writer: Box::new(tokio::io::stderr()),
            }
        }
    }

    fn finish(
        self: Box<Self>,
        _status: Option<std::process::ExitStatus>,
        _cache_update_status: CacheUpdateStatus,
        error: Option<ExecutionError>,
    ) {
        let mut stdout = std::io::stdout();

        // Handle errors — format the full error chain and print inline.
        if let Some(error) = error {
            let message = vite_str::format!("{:#}", anyhow::Error::from(error));
            write_error_message(&mut stdout, &message);
            return;
        }

        // For cache hits, print the "cache hit" message (unless silent)
        if self.is_cache_hit && !self.is_silent() {
            write_cache_hit_message(&mut stdout);
        }
    }
}

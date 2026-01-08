//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect NO_COLOR environment variable.

use super::{CacheMiss, FingerprintMismatch};
use crate::session::event::{CacheDisabledReason, CacheStatus};

/// Format cache status for inline display (during Start event).
///
/// Returns Some(formatted_string) for Hit and Miss with reason, None otherwise.
/// - Cache Hit: Shows "cache hit" indicator
/// - Cache Miss (NotFound): No inline message (just command)
/// - Cache Miss (with mismatch): Shows "cache miss" with brief reason
/// - Cache Disabled: No inline message
///
/// Note: Returns plain text without styling. The reporter applies colors.
pub fn format_cache_status_inline(cache_status: &CacheStatus) -> Option<String> {
    match cache_status {
        CacheStatus::Hit { .. } => {
            // Show "cache hit" indicator when replaying from cache
            Some("(✓ cache hit, replaying)".to_string())
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // No inline message for "not found" case - just show command
            // This keeps the output clean for first-time executions
            None
        }
        CacheStatus::Miss(CacheMiss::FingerprintMismatch(mismatch)) => {
            // Show "cache miss" with brief reason why cache couldn't be used
            // Detailed diff is shown in the summary section
            let reason = match mismatch {
                FingerprintMismatch::SpawnFingerprintMismatch(_previous) => {
                    // Simplified inline message - detailed diff shown in summary
                    "command configuration changed"
                }
                FingerprintMismatch::PostRunFingerprintMismatch(_diff) => {
                    // Simplified inline message - detailed diff shown in summary
                    "input files changed"
                }
            };
            Some(format!("(✗ cache miss: {}, executing)", reason))
        }
        CacheStatus::Disabled(_) => {
            // No inline message for disabled cache - keeps output clean
            None
        }
    }
}

/// Format cache status for summary display (post-execution).
///
/// Returns a formatted string showing detailed cache information.
/// - Cache Hit: Shows saved time
/// - Cache Miss (NotFound): Indicates first-time execution
/// - Cache Miss (with mismatch): Shows specific reason with details
/// - Cache Disabled: Shows user-friendly reason message
///
/// Note: Returns plain text without styling. The reporter applies colors.
pub fn format_cache_status_summary(cache_status: &CacheStatus) -> String {
    match cache_status {
        CacheStatus::Hit { replayed_duration } => {
            // Show saved time for cache hits
            format!("→ Cache hit - output replayed - {replayed_duration:.2?} saved")
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // First time running this task - no previous cache entry
            "→ Cache miss: no previous cache entry found".to_string()
        }
        CacheStatus::Miss(CacheMiss::FingerprintMismatch(mismatch)) => {
            // Show specific reason why cache was invalidated
            match mismatch {
                FingerprintMismatch::SpawnFingerprintMismatch(_previous_fingerprint) => {
                    // For spawn fingerprint mismatch, we would need the current fingerprint
                    // to show detailed "from X to Y" diffs. For now, show a generic message.
                    // TODO: Consider passing current fingerprint to enable detailed diffs
                    "→ Cache miss: command configuration changed".to_string()
                }
                FingerprintMismatch::PostRunFingerprintMismatch(diff) => {
                    // Post-run mismatch has specific path information
                    use crate::session::execute::fingerprint::PostRunFingerprintMismatch;
                    match diff {
                        PostRunFingerprintMismatch::InputContentChanged { path } => {
                            format!("→ Cache miss: content of input '{path}' changed")
                        }
                    }
                }
            }
        }
        CacheStatus::Disabled(reason) => {
            // Display user-friendly message for each disabled reason
            let message = match reason {
                CacheDisabledReason::InProcessExecution => "Cache disabled for built-in command",
                CacheDisabledReason::NoCacheMetadata => "Cache disabled in task configuration",
                CacheDisabledReason::CycleDetected => "Cache disabled: cycle detected",
            };
            format!("→ {message}")
        }
    }
}

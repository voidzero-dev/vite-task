//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect NO_COLOR environment variable.

use vite_task_plan::cache_metadata::SpawnFingerprint;

use super::{CacheMiss, FingerprintMismatch};
use crate::session::event::{CacheDisabledReason, CacheStatus};

/// Describes what changed between two spawn fingerprints
enum SpawnFingerprintChange {
    /// Environment variable value changed
    EnvValueChanged { key: String, old_value: String, new_value: String },
    /// Pass-through environment configuration changed
    PassThroughEnvConfigChanged { old_config: String, new_config: String },
    /// Command changed (program or args)
    CommandChanged,
    /// Working directory changed
    CwdChanged,
    /// Fingerprint ignores configuration changed
    FingerprintIgnoresChanged,
    /// Multiple changes or couldn't determine specific change
    MultipleChanges,
}

/// Compare two spawn fingerprints and determine what changed
fn detect_spawn_fingerprint_change(
    old: &SpawnFingerprint,
    new: &SpawnFingerprint,
) -> SpawnFingerprintChange {
    let old_env = old.env_fingerprints();
    let new_env = new.env_fingerprints();

    // Check for env value changes
    let mut env_changes = Vec::new();
    for (key, old_value) in &old_env.fingerprinted_envs {
        if let Some(new_value) = new_env.fingerprinted_envs.get(key) {
            if old_value != new_value {
                env_changes.push((key.to_string(), old_value.to_string(), new_value.to_string()));
            }
        } else {
            // Key was removed
            env_changes.push((key.to_string(), old_value.to_string(), "<removed>".to_string()));
        }
    }
    // Check for new keys
    for (key, new_value) in &new_env.fingerprinted_envs {
        if !old_env.fingerprinted_envs.contains_key(key) {
            env_changes.push((key.to_string(), "<not set>".to_string(), new_value.to_string()));
        }
    }

    // Check for pass-through env config changes
    let pass_through_changed = old_env.pass_through_env_config != new_env.pass_through_env_config;

    // Check for command changes (program or args)
    let command_changed = old.program_fingerprint_debug() != new.program_fingerprint_debug()
        || old.args() != new.args();

    // Check for cwd changes
    let cwd_changed = old.cwd() != new.cwd();

    // Check for fingerprint ignores changes
    let fingerprint_ignores_changed = old.fingerprint_ignores() != new.fingerprint_ignores();

    // Determine the most specific change
    let change_count = (if env_changes.is_empty() { 0 } else { 1 })
        + pass_through_changed as usize
        + command_changed as usize
        + cwd_changed as usize
        + fingerprint_ignores_changed as usize;

    if change_count == 0 {
        // Shouldn't happen, but handle gracefully
        SpawnFingerprintChange::MultipleChanges
    } else if !env_changes.is_empty() && change_count == 1 {
        // Only env changes - report the first one
        let (key, old_val, new_val) = env_changes.into_iter().next().unwrap();
        SpawnFingerprintChange::EnvValueChanged { key, old_value: old_val, new_value: new_val }
    } else if pass_through_changed && change_count == 1 {
        SpawnFingerprintChange::PassThroughEnvConfigChanged {
            old_config: format!("{:?}", old_env.pass_through_env_config),
            new_config: format!("{:?}", new_env.pass_through_env_config),
        }
    } else if command_changed && change_count == 1 {
        SpawnFingerprintChange::CommandChanged
    } else if cwd_changed && change_count == 1 {
        SpawnFingerprintChange::CwdChanged
    } else if fingerprint_ignores_changed && change_count == 1 {
        SpawnFingerprintChange::FingerprintIgnoresChanged
    } else {
        SpawnFingerprintChange::MultipleChanges
    }
}

/// Format cache status for inline display (during Start event).
///
/// Returns Some(formatted_string) for Hit, Miss with reason, and Disabled, None for NotFound.
/// - Cache Hit: Shows "cache hit" indicator
/// - Cache Miss (NotFound): No inline message (just command)
/// - Cache Miss (with mismatch): Shows "cache miss" with brief reason
/// - Cache Disabled: Shows "cache disabled" with reason
///
/// Note: Returns plain text without styling. The reporter applies colors.
pub fn format_cache_status_inline(cache_status: &CacheStatus) -> Option<String> {
    match cache_status {
        CacheStatus::Hit { .. } => {
            // Show "cache hit" indicator when replaying from cache
            Some("✓ cache hit, replaying".to_string())
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // No inline message for "not found" case - just show command
            // This keeps the output clean for first-time executions
            None
        }
        CacheStatus::Miss(CacheMiss::FingerprintMismatch(mismatch)) => {
            // Show "cache miss" with reason why cache couldn't be used
            let reason = match mismatch {
                FingerprintMismatch::SpawnFingerprintMismatch { old, new } => {
                    match detect_spawn_fingerprint_change(old, new) {
                        SpawnFingerprintChange::EnvValueChanged { .. } => {
                            "envs changed".to_string()
                        }
                        SpawnFingerprintChange::PassThroughEnvConfigChanged { .. } => {
                            "pass-through env config changed".to_string()
                        }
                        SpawnFingerprintChange::CommandChanged => "command changed".to_string(),
                        SpawnFingerprintChange::CwdChanged => {
                            "working directory changed".to_string()
                        }
                        SpawnFingerprintChange::FingerprintIgnoresChanged => {
                            "fingerprint ignores changed".to_string()
                        }
                        SpawnFingerprintChange::MultipleChanges => {
                            "configuration changed".to_string()
                        }
                    }
                }
                FingerprintMismatch::PostRunFingerprintMismatch(diff) => {
                    use crate::session::execute::fingerprint::PostRunFingerprintMismatch;
                    match diff {
                        PostRunFingerprintMismatch::InputContentChanged { path } => {
                            format!("content of input '{path}' changed")
                        }
                    }
                }
            };
            Some(format!("✗ cache miss: {reason}, executing"))
        }
        CacheStatus::Disabled(reason) => {
            // Show inline message for disabled cache
            let message = match reason {
                CacheDisabledReason::InProcessExecution => "cache disabled: built-in command",
                CacheDisabledReason::NoCacheMetadata => "cache disabled: no cache config",
                CacheDisabledReason::CycleDetected => "cache disabled: cycle detected",
            };
            Some(format!("⊘ {message}"))
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
                FingerprintMismatch::SpawnFingerprintMismatch { old, new } => {
                    match detect_spawn_fingerprint_change(old, new) {
                        SpawnFingerprintChange::EnvValueChanged { key, old_value, new_value } => {
                            format!(
                                "→ Cache miss: env {key} value changed from '{old_value}' to '{new_value}'"
                            )
                        }
                        SpawnFingerprintChange::PassThroughEnvConfigChanged { .. } => {
                            "→ Cache miss: pass-through env configuration changed".to_string()
                        }
                        SpawnFingerprintChange::CommandChanged => {
                            "→ Cache miss: command changed".to_string()
                        }
                        SpawnFingerprintChange::CwdChanged => {
                            "→ Cache miss: working directory changed".to_string()
                        }
                        SpawnFingerprintChange::FingerprintIgnoresChanged => {
                            "→ Cache miss: fingerprint ignores configuration changed".to_string()
                        }
                        SpawnFingerprintChange::MultipleChanges => {
                            "→ Cache miss: configuration changed".to_string()
                        }
                    }
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

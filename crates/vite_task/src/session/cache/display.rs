//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect `NO_COLOR` environment variable.

use std::collections::HashSet;

use vite_task_plan::cache_metadata::SpawnFingerprint;

use super::{CacheMiss, FingerprintMismatch};
use crate::session::event::{CacheDisabledReason, CacheStatus};

/// Describes a single atomic change between two spawn fingerprints
enum SpawnFingerprintChange {
    // Environment variable changes
    /// Environment variable added
    EnvAdded { key: String, value: String },
    /// Environment variable removed
    EnvRemoved { key: String, value: String },
    /// Environment variable value changed
    EnvValueChanged { key: String, old_value: String, new_value: String },

    // Pass-through env config changes
    /// Pass-through env pattern added
    PassThroughEnvAdded { name: String },
    /// Pass-through env pattern removed
    PassThroughEnvRemoved { name: String },

    // Command changes
    /// Program changed
    ProgramChanged,
    /// Args changed
    ArgsChanged,

    // Working directory change
    /// Working directory changed
    CwdChanged,

    // Fingerprint ignores changes
    /// Fingerprint ignore pattern added
    FingerprintIgnoreAdded { pattern: String },
    /// Fingerprint ignore pattern removed
    FingerprintIgnoreRemoved { pattern: String },
}

/// Compare two spawn fingerprints and return all changes
fn detect_spawn_fingerprint_changes(
    old: &SpawnFingerprint,
    new: &SpawnFingerprint,
) -> Vec<SpawnFingerprintChange> {
    let mut changes = Vec::new();
    let old_env = old.env_fingerprints();
    let new_env = new.env_fingerprints();

    // Check for removed or changed envs
    for (key, old_value) in &old_env.fingerprinted_envs {
        if let Some(new_value) = new_env.fingerprinted_envs.get(key) {
            if old_value != new_value {
                changes.push(SpawnFingerprintChange::EnvValueChanged {
                    key: key.to_string(),
                    old_value: old_value.to_string(),
                    new_value: new_value.to_string(),
                });
            }
        } else {
            changes.push(SpawnFingerprintChange::EnvRemoved {
                key: key.to_string(),
                value: old_value.to_string(),
            });
        }
    }

    // Check for added envs
    for (key, new_value) in &new_env.fingerprinted_envs {
        if !old_env.fingerprinted_envs.contains_key(key) {
            changes.push(SpawnFingerprintChange::EnvAdded {
                key: key.to_string(),
                value: new_value.to_string(),
            });
        }
    }

    // Check pass-through env config changes
    let old_pass_through: HashSet<_> = old_env.pass_through_env_config.iter().collect();
    let new_pass_through: HashSet<_> = new_env.pass_through_env_config.iter().collect();
    for name in old_pass_through.difference(&new_pass_through) {
        changes.push(SpawnFingerprintChange::PassThroughEnvRemoved { name: name.to_string() });
    }
    for name in new_pass_through.difference(&old_pass_through) {
        changes.push(SpawnFingerprintChange::PassThroughEnvAdded { name: name.to_string() });
    }

    // Check program changes
    if old.program_fingerprint_debug() != new.program_fingerprint_debug() {
        changes.push(SpawnFingerprintChange::ProgramChanged);
    }

    // Check args changes
    if old.args() != new.args() {
        changes.push(SpawnFingerprintChange::ArgsChanged);
    }

    // Check cwd changes
    if old.cwd() != new.cwd() {
        changes.push(SpawnFingerprintChange::CwdChanged);
    }

    // Check fingerprint ignores changes
    let old_ignores: HashSet<_> =
        old.fingerprint_ignores().map(|v| v.iter().collect()).unwrap_or_default();
    let new_ignores: HashSet<_> =
        new.fingerprint_ignores().map(|v| v.iter().collect()).unwrap_or_default();
    for pattern in old_ignores.difference(&new_ignores) {
        changes.push(SpawnFingerprintChange::FingerprintIgnoreRemoved {
            pattern: pattern.to_string(),
        });
    }
    for pattern in new_ignores.difference(&old_ignores) {
        changes
            .push(SpawnFingerprintChange::FingerprintIgnoreAdded { pattern: pattern.to_string() });
    }

    changes
}

/// Format cache status for inline display (during Start event).
///
/// Returns `Some(formatted_string)` for Hit, Miss with reason, and Disabled, None for `NotFound`.
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
                    let changes = detect_spawn_fingerprint_changes(old, new);
                    match changes.first() {
                        Some(
                            SpawnFingerprintChange::EnvAdded { .. }
                            | SpawnFingerprintChange::EnvRemoved { .. }
                            | SpawnFingerprintChange::EnvValueChanged { .. },
                        ) => "envs changed",
                        Some(
                            SpawnFingerprintChange::PassThroughEnvAdded { .. }
                            | SpawnFingerprintChange::PassThroughEnvRemoved { .. },
                        ) => "pass-through env config changed",
                        Some(SpawnFingerprintChange::ProgramChanged) => "program changed",
                        Some(SpawnFingerprintChange::ArgsChanged) => "args changed",
                        Some(SpawnFingerprintChange::CwdChanged) => "working directory changed",
                        Some(
                            SpawnFingerprintChange::FingerprintIgnoreAdded { .. }
                            | SpawnFingerprintChange::FingerprintIgnoreRemoved { .. },
                        ) => "fingerprint ignores changed",
                        None => "configuration changed",
                    }
                }
                FingerprintMismatch::PostRunFingerprintMismatch(diff) => {
                    use crate::session::execute::fingerprint::PostRunFingerprintMismatch;
                    match diff {
                        PostRunFingerprintMismatch::InputContentChanged { path } => {
                            return Some(format!(
                                "✗ cache miss: content of input '{path}' changed, executing"
                            ));
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
                    let changes = detect_spawn_fingerprint_changes(old, new);
                    let formatted: Vec<String> = changes
                        .iter()
                        .map(|c| match c {
                            SpawnFingerprintChange::EnvAdded { key, value } => {
                                format!("env {key}={value} added")
                            }
                            SpawnFingerprintChange::EnvRemoved { key, value } => {
                                format!("env {key}={value} removed")
                            }
                            SpawnFingerprintChange::EnvValueChanged {
                                key,
                                old_value,
                                new_value,
                            } => {
                                format!(
                                    "env {key} value changed from '{old_value}' to '{new_value}'"
                                )
                            }
                            SpawnFingerprintChange::PassThroughEnvAdded { name } => {
                                format!("pass-through env '{name}' added")
                            }
                            SpawnFingerprintChange::PassThroughEnvRemoved { name } => {
                                format!("pass-through env '{name}' removed")
                            }
                            SpawnFingerprintChange::ProgramChanged => "program changed".to_string(),
                            SpawnFingerprintChange::ArgsChanged => "args changed".to_string(),
                            SpawnFingerprintChange::CwdChanged => {
                                "working directory changed".to_string()
                            }
                            SpawnFingerprintChange::FingerprintIgnoreAdded { pattern } => {
                                format!("fingerprint ignore '{pattern}' added")
                            }
                            SpawnFingerprintChange::FingerprintIgnoreRemoved { pattern } => {
                                format!("fingerprint ignore '{pattern}' removed")
                            }
                        })
                        .collect();

                    if formatted.is_empty() {
                        "→ Cache miss: configuration changed".to_string()
                    } else {
                        format!("→ Cache miss: {}", formatted.join("; "))
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

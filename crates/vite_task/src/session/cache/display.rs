//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect `NO_COLOR` environment variable.

use rustc_hash::FxHashSet;
use vite_str::Str;
use vite_task_plan::cache_metadata::SpawnFingerprint;

use super::{CacheMiss, FingerprintMismatch};
use crate::session::event::{CacheDisabledReason, CacheStatus};

/// Describes a single atomic change between two spawn fingerprints
enum SpawnFingerprintChange {
    // Environment variable changes
    /// Environment variable added
    EnvAdded { key: Str, value: Str },
    /// Environment variable removed
    EnvRemoved { key: Str, value: Str },
    /// Environment variable value changed
    EnvValueChanged { key: Str, old_value: Str, new_value: Str },

    // Pass-through env config changes
    /// Pass-through env pattern added
    PassThroughEnvAdded { name: Str },
    /// Pass-through env pattern removed
    PassThroughEnvRemoved { name: Str },

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
    FingerprintIgnoreAdded { pattern: Str },
    /// Fingerprint ignore pattern removed
    FingerprintIgnoreRemoved { pattern: Str },
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
                    key: key.clone(),
                    old_value: Str::from(old_value.as_ref()),
                    new_value: Str::from(new_value.as_ref()),
                });
            }
        } else {
            changes.push(SpawnFingerprintChange::EnvRemoved {
                key: key.clone(),
                value: Str::from(old_value.as_ref()),
            });
        }
    }

    // Check for added envs
    for (key, new_value) in &new_env.fingerprinted_envs {
        if !old_env.fingerprinted_envs.contains_key(key) {
            changes.push(SpawnFingerprintChange::EnvAdded {
                key: key.clone(),
                value: Str::from(new_value.as_ref()),
            });
        }
    }

    // Check pass-through env config changes
    let old_pass_through: FxHashSet<_> = old_env.pass_through_env_config.iter().collect();
    let new_pass_through: FxHashSet<_> = new_env.pass_through_env_config.iter().collect();
    for name in old_pass_through.difference(&new_pass_through) {
        changes.push(SpawnFingerprintChange::PassThroughEnvRemoved { name: (*name).clone() });
    }
    for name in new_pass_through.difference(&old_pass_through) {
        changes.push(SpawnFingerprintChange::PassThroughEnvAdded { name: (*name).clone() });
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
    let old_ignores: FxHashSet<_> =
        old.fingerprint_ignores().map(|v| v.iter().collect()).unwrap_or_default();
    let new_ignores: FxHashSet<_> =
        new.fingerprint_ignores().map(|v| v.iter().collect()).unwrap_or_default();
    for pattern in old_ignores.difference(&new_ignores) {
        changes
            .push(SpawnFingerprintChange::FingerprintIgnoreRemoved { pattern: (*pattern).clone() });
    }
    for pattern in new_ignores.difference(&old_ignores) {
        changes
            .push(SpawnFingerprintChange::FingerprintIgnoreAdded { pattern: (*pattern).clone() });
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
pub fn format_cache_status_inline(cache_status: &CacheStatus) -> Option<Str> {
    match cache_status {
        CacheStatus::Hit { .. } => {
            // Show "cache hit" indicator when replaying from cache
            Some(Str::from("✓ cache hit, replaying"))
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
                            return Some(vite_str::format!(
                                "✗ cache miss: content of input '{path}' changed, executing"
                            ));
                        }
                    }
                }
            };
            Some(vite_str::format!("✗ cache miss: {reason}, executing"))
        }
        CacheStatus::Disabled(reason) => {
            // Show inline message for disabled cache
            let message = match reason {
                CacheDisabledReason::InProcessExecution => "cache disabled: built-in command",
                CacheDisabledReason::NoCacheMetadata => "cache disabled: no cache config",
            };
            Some(vite_str::format!("⊘ {message}"))
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
pub fn format_cache_status_summary(cache_status: &CacheStatus) -> Str {
    match cache_status {
        CacheStatus::Hit { replayed_duration } => {
            // Show saved time for cache hits
            vite_str::format!("→ Cache hit - output replayed - {replayed_duration:.2?} saved")
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // First time running this task - no previous cache entry
            Str::from("→ Cache miss: no previous cache entry found")
        }
        CacheStatus::Miss(CacheMiss::FingerprintMismatch(mismatch)) => {
            // Show specific reason why cache was invalidated
            match mismatch {
                FingerprintMismatch::SpawnFingerprintMismatch { old, new } => {
                    let changes = detect_spawn_fingerprint_changes(old, new);
                    let formatted: Vec<Str> = changes
                        .iter()
                        .map(|c| match c {
                            SpawnFingerprintChange::EnvAdded { key, value } => {
                                vite_str::format!("env {key}={value} added")
                            }
                            SpawnFingerprintChange::EnvRemoved { key, value } => {
                                vite_str::format!("env {key}={value} removed")
                            }
                            SpawnFingerprintChange::EnvValueChanged {
                                key,
                                old_value,
                                new_value,
                            } => {
                                vite_str::format!(
                                    "env {key} value changed from '{old_value}' to '{new_value}'"
                                )
                            }
                            SpawnFingerprintChange::PassThroughEnvAdded { name } => {
                                vite_str::format!("pass-through env '{name}' added")
                            }
                            SpawnFingerprintChange::PassThroughEnvRemoved { name } => {
                                vite_str::format!("pass-through env '{name}' removed")
                            }
                            SpawnFingerprintChange::ProgramChanged => Str::from("program changed"),
                            SpawnFingerprintChange::ArgsChanged => Str::from("args changed"),
                            SpawnFingerprintChange::CwdChanged => {
                                Str::from("working directory changed")
                            }
                            SpawnFingerprintChange::FingerprintIgnoreAdded { pattern } => {
                                vite_str::format!("fingerprint ignore '{pattern}' added")
                            }
                            SpawnFingerprintChange::FingerprintIgnoreRemoved { pattern } => {
                                vite_str::format!("fingerprint ignore '{pattern}' removed")
                            }
                        })
                        .collect();

                    if formatted.is_empty() {
                        Str::from("→ Cache miss: configuration changed")
                    } else {
                        let joined =
                            formatted.iter().map(Str::as_str).collect::<Vec<_>>().join("; ");
                        vite_str::format!("→ Cache miss: {joined}")
                    }
                }
                FingerprintMismatch::PostRunFingerprintMismatch(diff) => {
                    // Post-run mismatch has specific path information
                    use crate::session::execute::fingerprint::PostRunFingerprintMismatch;
                    match diff {
                        PostRunFingerprintMismatch::InputContentChanged { path } => {
                            vite_str::format!("→ Cache miss: content of input '{path}' changed")
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
            };
            vite_str::format!("→ {message}")
        }
    }
}

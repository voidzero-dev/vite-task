//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect `NO_COLOR` environment variable.

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use vite_str::Str;
use vite_task_plan::cache_metadata::SpawnFingerprint;

use super::{CacheMiss, EnvMismatch, FingerprintMismatch, InputChangeKind, split_path};
use crate::session::event::CacheStatus;

/// Describes a single atomic change between two spawn fingerprints.
///
/// Used both for live cache status display and for persisted summary data.
#[derive(Serialize, Deserialize)]
pub enum SpawnFingerprintChange {
    /// A fingerprinted env var was added, removed, or changed value.
    Env(EnvMismatch),

    // Untracked env config changes
    /// Untracked env pattern added
    UntrackedEnvAdded { name: Str },
    /// Untracked env pattern removed
    UntrackedEnvRemoved { name: Str },

    // Command changes
    /// Program changed
    ProgramChanged,
    /// Args changed
    ArgsChanged,

    // Working directory change
    /// Working directory changed
    CwdChanged,
}

/// Format a single spawn fingerprint change as human-readable text.
///
/// Used by both the live cache status display and the persisted summary rendering.
pub fn format_spawn_change(change: &SpawnFingerprintChange) -> Str {
    match change {
        SpawnFingerprintChange::Env(mismatch) => vite_str::format!("{mismatch}"),
        SpawnFingerprintChange::UntrackedEnvAdded { name } => {
            vite_str::format!("untracked env '{name}' added")
        }
        SpawnFingerprintChange::UntrackedEnvRemoved { name } => {
            vite_str::format!("untracked env '{name}' removed")
        }
        SpawnFingerprintChange::ProgramChanged => Str::from("program changed"),
        SpawnFingerprintChange::ArgsChanged => Str::from("args changed"),
        SpawnFingerprintChange::CwdChanged => Str::from("working directory changed"),
    }
}

/// Compare two spawn fingerprints and return all changes.
pub fn detect_spawn_fingerprint_changes(
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
                changes
                    .push(SpawnFingerprintChange::Env(EnvMismatch::Changed { name: key.clone() }));
            }
        } else {
            changes.push(SpawnFingerprintChange::Env(EnvMismatch::Removed { name: key.clone() }));
        }
    }

    // Check for added envs
    for key in new_env.fingerprinted_envs.keys() {
        if !old_env.fingerprinted_envs.contains_key(key) {
            changes.push(SpawnFingerprintChange::Env(EnvMismatch::Added { name: key.clone() }));
        }
    }

    // Check untracked env config changes
    let old_untracked: FxHashSet<_> = old_env.untracked_env_config.iter().collect();
    let new_untracked: FxHashSet<_> = new_env.untracked_env_config.iter().collect();
    for name in old_untracked.difference(&new_untracked) {
        changes.push(SpawnFingerprintChange::UntrackedEnvRemoved { name: (*name).clone() });
    }
    for name in new_untracked.difference(&old_untracked) {
        changes.push(SpawnFingerprintChange::UntrackedEnvAdded { name: (*name).clone() });
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

    changes
}

/// Names of the env vars involved in a set of spawn-fingerprint changes, in the
/// order detected. Only env changes are collected; untracked-env and non-env
/// changes are skipped.
fn env_change_names(changes: &[SpawnFingerprintChange]) -> Vec<&Str> {
    changes
        .iter()
        .filter_map(|change| match change {
            SpawnFingerprintChange::Env(mismatch) => Some(mismatch.name()),
            _ => None,
        })
        .collect()
}

/// Inline cache-miss reason naming the env var(s) that changed, e.g.
/// `env 'NODE_ENV' changed` or `envs 'A', 'B' changed`. Falls back to the
/// generic `envs changed` when no names are available.
fn format_env_changed_inline(names: &[&Str]) -> Str {
    match names {
        [] => Str::from("envs changed"),
        [name] => vite_str::format!("env '{name}' changed"),
        names => {
            let quoted: Vec<Str> = names.iter().map(|name| vite_str::format!("'{name}'")).collect();
            let joined = quoted.iter().map(Str::as_str).collect::<Vec<_>>().join(", ");
            vite_str::format!("envs {joined} changed")
        }
    }
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
            Some(Str::from("◉ cache hit, replaying"))
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // No inline message for "not found" case - just show command
            // This keeps the output clean for first-time executions
            None
        }
        CacheStatus::Miss(CacheMiss::FingerprintMismatch(mismatch)) => {
            // Show "cache miss" with reason why cache couldn't be used
            let reason = match mismatch {
                FingerprintMismatch::SpawnFingerprint { old, new } => {
                    let changes = detect_spawn_fingerprint_changes(old, new);
                    match changes.first() {
                        Some(SpawnFingerprintChange::Env(_)) => {
                            format_env_changed_inline(&env_change_names(&changes))
                        }
                        Some(
                            SpawnFingerprintChange::UntrackedEnvAdded { .. }
                            | SpawnFingerprintChange::UntrackedEnvRemoved { .. },
                        ) => Str::from("untracked env config changed"),
                        Some(SpawnFingerprintChange::ProgramChanged) => {
                            Str::from("program changed")
                        }
                        Some(SpawnFingerprintChange::ArgsChanged) => Str::from("args changed"),
                        Some(SpawnFingerprintChange::CwdChanged) => {
                            Str::from("working directory changed")
                        }
                        None => Str::from("configuration changed"),
                    }
                }
                FingerprintMismatch::InputConfig => Str::from("input configuration changed"),
                FingerprintMismatch::OutputConfig => Str::from("output configuration changed"),
                FingerprintMismatch::InputChanged { kind, path } => {
                    format_input_change_str(*kind, path.as_str())
                }
                FingerprintMismatch::TrackedEnvChanged(mismatch) => {
                    format_env_changed_inline(&[mismatch.name()])
                }
            };
            Some(vite_str::format!("○ cache miss: {reason}, executing"))
        }
        CacheStatus::Disabled(_) => Some(Str::from("⊘ cache disabled")),
    }
}

/// Format an input change as a [`Str`] for inline display.
pub fn format_input_change_str(kind: InputChangeKind, path: &str) -> Str {
    match kind {
        InputChangeKind::ContentModified => vite_str::format!("'{path}' modified"),
        InputChangeKind::Added => {
            let (dir, filename) = split_path(path);
            dir.map_or_else(
                || vite_str::format!("'{filename}' added in workspace root"),
                |dir| vite_str::format!("'{filename}' added in '{dir}'"),
            )
        }
        InputChangeKind::Removed => {
            let (dir, filename) = split_path(path);
            dir.map_or_else(
                || vite_str::format!("'{filename}' removed from workspace root"),
                |dir| vite_str::format!("'{filename}' removed from '{dir}'"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_spawn_changes_report_names_only() {
        let added = SpawnFingerprintChange::Env(EnvMismatch::Added { name: Str::from("MY_ENV") });
        let removed =
            SpawnFingerprintChange::Env(EnvMismatch::Removed { name: Str::from("MY_ENV") });
        let changed =
            SpawnFingerprintChange::Env(EnvMismatch::Changed { name: Str::from("MY_ENV") });

        assert_eq!(format_spawn_change(&added).as_str(), "env 'MY_ENV' added");
        assert_eq!(format_spawn_change(&removed).as_str(), "env 'MY_ENV' removed");
        assert_eq!(format_spawn_change(&changed).as_str(), "env 'MY_ENV' changed");
    }

    #[test]
    fn inline_env_change_reason_reports_names_only() {
        let first = Str::from("API_KEY");
        let second = Str::from("NODE_ENV");

        assert_eq!(
            format_env_changed_inline(&[&first, &second]).as_str(),
            "envs 'API_KEY', 'NODE_ENV' changed"
        );
    }
}

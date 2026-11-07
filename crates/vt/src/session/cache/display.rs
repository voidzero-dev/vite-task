//! Human-readable formatting for cache status
//!
//! This module provides plain text formatting for cache status.
//! Coloring is handled by the reporter to respect `NO_COLOR` environment variable.

use vt_str::Str;

use super::{CacheHitSource, CacheMiss, Change, EnvSource, InputChangeKind, split_path};
use crate::session::event::{CacheStatus, RunKind};

/// Format one change as the detailed summary lists it, e.g.
/// `env 'NODE_ENV' added` or `'src/a.ts' modified`.
pub fn format_change(change: &Change) -> Str {
    match change {
        Change::Program { .. } => Str::from("program changed"),
        Change::Args { .. } => Str::from("args changed"),
        Change::Cwd { .. } => Str::from("working directory changed"),
        Change::InputConfig => Str::from("input configuration changed"),
        Change::OutputConfig => Str::from("output configuration changed"),
        Change::UntrackedEnvAdded { name } => vt_str::format!("untracked env '{name}' added"),
        Change::UntrackedEnvRemoved { name } => vt_str::format!("untracked env '{name}' removed"),
        Change::Env { mismatch, .. } => vt_str::format!("{mismatch}"),
        Change::File { kind, path } => format_input_change_str(*kind, path.as_str()),
    }
}

/// The inline reason for a miss: its first change. When the first change is
/// an env, every env change is named, as in `envs 'A', 'B' changed`, and a
/// single env from `cache.env` reads `env 'NODE_ENV' changed` however it
/// changed. Untracked env changes read `untracked env config changed`.
fn format_changes_inline(changes: &[Change]) -> Str {
    match changes.first() {
        None => Str::from("configuration changed"),
        Some(Change::UntrackedEnvAdded { .. } | Change::UntrackedEnvRemoved { .. }) => {
            Str::from("untracked env config changed")
        }
        Some(Change::Env { mismatch, source }) => {
            let names: Vec<&Str> = changes
                .iter()
                .filter_map(|change| match change {
                    Change::Env { mismatch, .. } => Some(mismatch.name()),
                    _ => None,
                })
                .collect();
            match (names.as_slice(), source) {
                ([_], EnvSource::Tracked) => vt_str::format!("{mismatch}"),
                _ => format_env_changed_inline(&names),
            }
        }
        Some(change) => format_change(change),
    }
}

/// Inline cache-miss reason naming the env var(s) that changed, e.g.
/// `env 'NODE_ENV' changed` or `envs 'A', 'B' changed`. Falls back to the
/// generic `envs changed` when no names are available.
fn format_env_changed_inline(names: &[&Str]) -> Str {
    match names {
        [] => Str::from("envs changed"),
        [name] => vt_str::format!("env '{name}' changed"),
        names => {
            let quoted: Vec<Str> = names.iter().map(|name| vt_str::format!("'{name}'")).collect();
            let joined = quoted.iter().map(Str::as_str).collect::<Vec<_>>().join(", ");
            vt_str::format!("envs {joined} changed")
        }
    }
}

/// Format cache status for inline display (during Start event).
///
/// Returns `Some(formatted_string)` for Hit, Miss with reason, and Disabled, None for `NotFound`.
/// - Cache Hit: Shows "cache hit" indicator, or "remote cache hit" for a remote hit
/// - Cache Miss (NotFound): No inline message (just command)
/// - Cache Miss (with mismatch): Shows "cache miss" with brief reason
/// - Cache Disabled: Shows "cache disabled" with reason
///
/// In a real run, a hit ends in ", replaying" and a miss in ", executing". A
/// dry run does neither, so its status has no such suffix.
///
/// Note: Returns plain text without styling. The reporter applies colors.
pub fn format_cache_status_inline(cache_status: &CacheStatus, run_kind: RunKind) -> Option<Str> {
    let suffix = |next_step: &str| match run_kind {
        RunKind::Real => vt_str::format!(", {next_step}"),
        RunKind::Dry => Str::default(),
    };
    match cache_status {
        CacheStatus::Hit { source, .. } => {
            // Show "cache hit" indicator when replaying from cache
            let hit = match source {
                CacheHitSource::Local => "cache hit",
                CacheHitSource::Remote => "remote cache hit",
            };
            Some(vt_str::format!("◉ {hit}{}", suffix("replaying")))
        }
        CacheStatus::Miss(CacheMiss::NotFound) => {
            // No inline message for "not found" case - just show command
            // This keeps the output clean for first-time executions
            None
        }
        CacheStatus::Miss(CacheMiss::Changed(comparison)) => {
            // Show "cache miss" with reason why cache couldn't be used
            let reason = format_changes_inline(&comparison.changes);
            Some(vt_str::format!("○ cache miss: {reason}{}", suffix("executing")))
        }
        CacheStatus::Miss(CacheMiss::RemoteReadFailed(error)) => {
            Some(vt_str::format!("○ cache miss: {error}{}", suffix("executing")))
        }
        CacheStatus::Disabled(_) => Some(Str::from("⊘ cache disabled")),
    }
}

/// Format an input change as a [`Str`] for inline display.
pub fn format_input_change_str(kind: InputChangeKind, path: &str) -> Str {
    match kind {
        InputChangeKind::ContentModified => vt_str::format!("'{path}' modified"),
        InputChangeKind::Added => {
            let (dir, filename) = split_path(path);
            dir.map_or_else(
                || vt_str::format!("'{filename}' added in workspace root"),
                |dir| vt_str::format!("'{filename}' added in '{dir}'"),
            )
        }
        InputChangeKind::Removed => {
            let (dir, filename) = split_path(path);
            dir.map_or_else(
                || vt_str::format!("'{filename}' removed from workspace root"),
                |dir| vt_str::format!("'{filename}' removed from '{dir}'"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::session::cache::{Comparison, EnvMismatch};

    fn env(mismatch: EnvMismatch, source: EnvSource) -> Change {
        Change::Env { mismatch, source }
    }

    fn miss(changes: Vec<Change>) -> CacheStatus {
        CacheStatus::Miss(CacheMiss::Changed(Comparison::new(changes, None, false)))
    }

    #[test]
    fn env_changes_report_names_only() {
        let name = || Str::from("MY_ENV");
        let source = EnvSource::Fingerprinted;
        let added = env(EnvMismatch::Added { name: name() }, source);
        let removed = env(EnvMismatch::Removed { name: name() }, source);
        let changed = env(EnvMismatch::Changed { name: name() }, source);

        assert_eq!(format_change(&added).as_str(), "env 'MY_ENV' added");
        assert_eq!(format_change(&removed).as_str(), "env 'MY_ENV' removed");
        assert_eq!(format_change(&changed).as_str(), "env 'MY_ENV' changed");
    }

    #[test]
    fn inline_reason_is_the_first_change_in_kind_order() {
        let fingerprinted =
            |name| env(EnvMismatch::Added { name: Str::from(name) }, EnvSource::Fingerprinted);
        let args = Change::Args { from: Arc::from([]), to: Arc::from([Str::from("--watch")]) };
        let status = miss(vec![fingerprinted("B"), args, fingerprinted("A")]);
        assert_eq!(
            format_cache_status_inline(&status, RunKind::Dry).as_deref(),
            Some("○ cache miss: args changed")
        );
        let status = miss(vec![fingerprinted("B"), fingerprinted("A")]);
        assert_eq!(
            format_cache_status_inline(&status, RunKind::Dry).as_deref(),
            Some("○ cache miss: envs 'A', 'B' changed")
        );
        let status = miss(vec![fingerprinted("A")]);
        assert_eq!(
            format_cache_status_inline(&status, RunKind::Dry).as_deref(),
            Some("○ cache miss: env 'A' changed")
        );
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

    #[test]
    fn inline_cache_hit_names_remote_source() {
        let hit = |source| CacheStatus::Hit {
            replayed_duration: std::time::Duration::ZERO,
            source,
            restores_outputs: false,
            baseline: None,
        };

        assert_eq!(
            format_cache_status_inline(&hit(CacheHitSource::Local), RunKind::Real).as_deref(),
            Some("◉ cache hit, replaying")
        );
        assert_eq!(
            format_cache_status_inline(&hit(CacheHitSource::Remote), RunKind::Real).as_deref(),
            Some("◉ remote cache hit, replaying")
        );
        assert_eq!(
            format_cache_status_inline(&hit(CacheHitSource::Remote), RunKind::Dry).as_deref(),
            Some("◉ remote cache hit")
        );
    }

    #[test]
    fn inline_tracked_env_mismatch_preserves_kind() {
        let added =
            miss(vec![env(EnvMismatch::Added { name: Str::from("PROBE_C") }, EnvSource::Tracked)]);
        let removed = miss(vec![env(
            EnvMismatch::Removed { name: Str::from("PROBE_A") },
            EnvSource::Tracked,
        )]);

        assert_eq!(
            format_cache_status_inline(&added, RunKind::Real).as_deref(),
            Some("○ cache miss: env 'PROBE_C' added, executing")
        );
        assert_eq!(
            format_cache_status_inline(&removed, RunKind::Dry).as_deref(),
            Some("○ cache miss: env 'PROBE_A' removed")
        );
    }
}

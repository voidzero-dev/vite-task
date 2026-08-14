//! Post-run cache update: decide whether a finished spawn may be cached and,
//! if so, store its fingerprints, captured output, and output archive.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_plan::cache_metadata::{CacheMetadata, EnvValueHash};
use vt_server::Reports;
use vt_str::Str;

use super::{
    CacheState,
    post_run::{TrackedEnvFingerprints, TrackedEnvQuery},
    spawn::ChildOutcome,
    task_fs::{Conclusion, PostRunError},
};
use crate::session::{
    cache::{CacheEntryValue, ExecutionCache, archive},
    event::{CacheErrorKind, CacheNotUpdatedReason, CacheUpdateStatus, ExecutionError},
};

type TrackedEnvValues = BTreeMap<Str, Option<EnvValueHash>>;
type TrackedEnvQueryValues = BTreeMap<TrackedEnvQuery, BTreeMap<Str, EnvValueHash>>;

/// Decide whether the finished run may be cached, and store it if so.
///
/// Every outcome returns a `(status, error)` pair for the caller's single
/// `finish()` call; this function never reports by itself. The guard clauses
/// run in priority order — each names the reason the run is *not* cached, and
/// only a run that passes them all is stored.
#[expect(
    clippy::too_many_arguments,
    reason = "the run's full context is genuinely needed to decide and store the cache entry"
)]
pub(super) async fn update_cache(
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    state: CacheState<'_>,
    outcome: &ChildOutcome,
    reports: Option<&Reports>,
    duration: Duration,
    cancelled: bool,
) -> (CacheUpdateStatus, Option<ExecutionError>) {
    let CacheState { metadata, task_fs, std_outputs, tracking } = state;

    if let Some(reports) = reports
        && reports.cache_disabled
    {
        // A runner-aware tool short-circuited caching via `disableCache()`
        // (e.g. a dev server with no deterministic output).
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::ToolRequested), None);
    }

    if cancelled {
        // Cancelled (Ctrl-C or sibling failure) — result is untrustworthy.
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::Cancelled), None);
    }

    if !outcome.exit_status.success() {
        // Execution failed with non-zero exit status — don't update cache.
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::NonZeroExitStatus), None);
    }

    // Task requested fspy auto-inference but no trace exists (this binary was
    // built without `cfg(fspy)`). Task ran, but we can't compute a valid cache
    // entry without tracked path accesses.
    #[cfg(fspy)]
    let has_trace = outcome.path_accesses.is_some();
    #[cfg(not(fspy))]
    let has_trace = false;
    if tracking.fspy && !has_trace {
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::FspyUnsupported), None);
    }

    let conclusion = {
        #[cfg(fspy)]
        let accesses = outcome.path_accesses.as_ref().map(fspy::PathAccessIterable::iter);
        #[cfg(not(fspy))]
        let accesses: Option<std::iter::Empty<fspy_shared::ipc::PathAccess<'_>>> = None;

        let empty_ignored = FxHashSet::default();
        let reported_ignored_inputs = reports.map_or(&empty_ignored, |r| &r.ignored_inputs);
        let reported_ignored_outputs = reports.map_or(&empty_ignored, |r| &r.ignored_outputs);
        task_fs.post_run(accesses, reported_ignored_inputs, reported_ignored_outputs)
    };
    let (input_fingerprints, outputs) = match conclusion {
        Ok(Conclusion::InputModified { path }) => {
            // The task wrote a file it also read, so the pre-run input hashes
            // are stale and caching is unsound.
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified { path }),
                None,
            );
        }
        Ok(Conclusion::Cacheable { input_fingerprints, outputs }) => (input_fingerprints, outputs),
        Err(PostRunError::InputFingerprints(err)) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::PostRunFingerprint(err)),
            );
        }
        Err(PostRunError::Outputs(err)) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Cache { kind: CacheErrorKind::Update, source: err }),
            );
        }
    };

    // Collect tool-reported tracked envs for the cache entry. Env names that
    // the user already declared are skipped because their values are already
    // part of the spawn fingerprint.
    let (tracked_envs, tracked_env_queries) = match collect_tracked_reports(reports, metadata) {
        Ok(tracked_reports) => tracked_reports,
        Err(err) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::PostRunFingerprint(err)),
            );
        }
    };

    let output_archive = match archive_outputs(&outputs, workspace_root, cache_dir) {
        Ok(archive) => archive,
        Err(err) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Cache { kind: CacheErrorKind::Update, source: err }),
            );
        }
    };

    let new_cache_value = CacheEntryValue {
        input_fingerprints,
        tracked_env_fingerprints: TrackedEnvFingerprints { tracked_envs, tracked_env_queries },
        std_outputs: std_outputs.into(),
        duration,
        output_archive,
    };
    match cache.update(metadata, new_cache_value, cache_dir).await {
        Ok(()) => (CacheUpdateStatus::Updated, None),
        Err(err) => (
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
            Some(ExecutionError::Cache { kind: CacheErrorKind::Update, source: err }),
        ),
    }
}

fn collect_tracked_reports(
    reports: Option<&Reports>,
    metadata: &CacheMetadata,
) -> anyhow::Result<(TrackedEnvValues, TrackedEnvQueryValues)> {
    reports
        .map(|reports| {
            let tracked_envs = collect_tracked_envs(reports, metadata)?;
            let tracked_env_queries = collect_tracked_env_queries(reports)?;
            Ok::<_, anyhow::Error>((tracked_envs, tracked_env_queries))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

/// Select tool-reported env records to embed in the cache entry.
/// Names that the user already declared as fingerprinted are skipped because
/// their values are already in the spawn fingerprint.
fn collect_tracked_envs(
    reports: &Reports,
    metadata: &CacheMetadata,
) -> anyhow::Result<TrackedEnvValues> {
    let fingerprinted = &metadata.spawn_fingerprint.env_fingerprints().fingerprinted_envs;
    let mut tracked_envs = BTreeMap::new();

    for (name, value) in &reports.tracked_get_env {
        let name_str =
            name.to_str().ok_or_else(|| anyhow::anyhow!("tracked env name is not valid UTF-8"))?;
        if fingerprinted.contains_key(name_str) {
            continue;
        }
        let value = value
            .as_ref()
            .map(|value| {
                let value_str = value.to_str().ok_or_else(|| {
                    anyhow::anyhow!("tracked env value for {name_str} is not valid UTF-8")
                })?;
                Ok::<_, anyhow::Error>(EnvValueHash::new(value_str))
            })
            .transpose()?;
        tracked_envs.insert(Str::from(name_str), value);
    }

    Ok(tracked_envs)
}

/// Select tool-reported bulk env query records to embed in the cache entry.
/// The full match-set is stored as value hashes.
fn collect_tracked_env_queries(reports: &Reports) -> anyhow::Result<TrackedEnvQueryValues> {
    let mut tracked_env_queries = BTreeMap::new();

    for (query, record) in &reports.tracked_get_envs {
        let mut matches = BTreeMap::new();
        for (name, value) in &record.matches {
            let name_str = name
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("tracked env match name is not valid UTF-8"))?;
            let value_str = value.to_str().ok_or_else(|| {
                anyhow::anyhow!("tracked env match value for {name_str} is not valid UTF-8")
            })?;
            matches.insert(Str::from(name_str), EnvValueHash::new(value_str));
        }
        let query = match query {
            vt_server::EnvQuery::Glob(pattern) => {
                TrackedEnvQuery::Glob(Str::from(pattern.as_ref()))
            }
            vt_server::EnvQuery::Prefix(prefix) => {
                TrackedEnvQuery::Prefix(Str::from(prefix.as_ref()))
            }
        };
        tracked_env_queries.insert(query, matches);
    }

    Ok(tracked_env_queries)
}

/// Archive the run's output files as a tar.zst in the cache directory.
///
/// Returns `Some(archive_filename)` if files were archived, `None` if the run
/// produced no output files.
fn archive_outputs(
    outputs: &[RelativePathBuf],
    workspace_root: &AbsolutePath,
    cache_dir: &AbsolutePath,
) -> anyhow::Result<Option<Str>> {
    if outputs.is_empty() {
        return Ok(None);
    }

    let archive_name: Str = vt_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
    let archive_path = cache_dir.join(archive_name.as_str());

    archive::create_output_archive(workspace_root, outputs, &archive_path)?;

    Ok(Some(archive_name))
}

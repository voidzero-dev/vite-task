//! Post-run cache update: decide whether a finished spawn may be cached and,
//! if so, store its fingerprint, captured output, and output archive.

use std::{sync::Arc, time::Duration};

use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_plan::cache_metadata::CacheMetadata;
use vite_task_server::Reports;

use super::{
    CacheState,
    fingerprint::{PathRead, PostRunFingerprint},
    glob,
    spawn::ChildOutcome,
};
use crate::{
    collections::HashMap,
    session::{
        cache::{CacheEntryValue, ExecutionCache, archive},
        event::{CacheErrorKind, CacheNotUpdatedReason, CacheUpdateStatus, ExecutionError},
    },
};

/// Post-execution summary of what fspy observed for a single task. Fields are
/// cfg-agnostic so the decision logic below doesn't need `cfg(fspy)` — the
/// value is only ever `Some` when tracking happened (see [`observe_fspy`]).
struct TrackingOutcome {
    path_reads: HashMap<RelativePathBuf, PathRead>,
    /// First path that was both read and written during execution, if any.
    /// A non-empty value means caching this task is unsound.
    read_write_overlap: Option<RelativePathBuf>,
}

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
    let CacheState { metadata, globbed_inputs, std_outputs, tracking } = state;
    let fspy = tracking.fspy.as_ref();
    let input_negative_globs = fspy.map(|t| t.input_negative_globs.as_slice());

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

    let fspy_outcome = observe_fspy(outcome, input_negative_globs, workspace_root);

    if let Some(TrackingOutcome { read_write_overlap: Some(path), .. }) = &fspy_outcome {
        // fspy-inferred read-write overlap: the task wrote to a file it also
        // read, so the prerun input hashes are stale and caching is unsound.
        // (We only check fspy-inferred reads, not globbed_inputs. A task that
        // writes to a glob-matched file without reading it produces perpetual
        // cache misses but not a correctness bug.)
        return (
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified {
                path: path.clone(),
            }),
            None,
        );
    }

    if fspy_outcome.is_none() && fspy.is_some() {
        // Task requested fspy auto-inference but this binary was built without
        // `cfg(fspy)`. Task ran, but we can't compute a valid cache entry
        // without tracked path accesses.
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::FspyUnsupported), None);
    }

    // Paths already in globbed_inputs are skipped: the overlap check above
    // guarantees no input modification, so the prerun hash is the correct
    // post-exec hash.
    let empty_path_reads = HashMap::default();
    let path_reads = fspy_outcome.as_ref().map_or(&empty_path_reads, |o| &o.path_reads);
    let post_run_fingerprint =
        match PostRunFingerprint::create(path_reads, workspace_root, &globbed_inputs) {
            Ok(fingerprint) => fingerprint,
            Err(err) => {
                return (
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::PostRunFingerprint(err)),
                );
            }
        };

    let output_archive = match collect_and_archive_outputs(metadata, workspace_root, cache_dir) {
        Ok(archive) => archive,
        Err(err) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Cache { kind: CacheErrorKind::Update, source: err }),
            );
        }
    };

    let new_cache_value = CacheEntryValue {
        post_run_fingerprint,
        std_outputs: std_outputs.into(),
        duration,
        globbed_inputs,
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

/// Summarize the run's fspy observations. `Some` iff tracking was both
/// requested (`input_negative_globs.is_some()`) and compiled in (`cfg(fspy)`). On a
/// `cfg(not(fspy))` build this is always `None`, and [`update_cache`]
/// short-circuits to `FspyUnsupported` when tracking was needed.
fn observe_fspy(
    outcome: &ChildOutcome,
    input_negative_globs: Option<&[wax::Glob<'static>]>,
    workspace_root: &AbsolutePath,
) -> Option<TrackingOutcome> {
    #[cfg(fspy)]
    {
        use super::tracked_accesses::TrackedPathAccesses;

        outcome.path_accesses.as_ref().zip(input_negative_globs).map(|(raw, negatives)| {
            let tracked = TrackedPathAccesses::from_raw(raw, workspace_root, negatives);
            let read_write_overlap =
                tracked.path_reads.keys().find(|p| tracked.path_writes.contains(*p)).cloned();
            TrackingOutcome { path_reads: tracked.path_reads, read_write_overlap }
        })
    }
    #[cfg(not(fspy))]
    {
        let _ = (outcome, input_negative_globs, workspace_root);
        None
    }
}

/// Collect output files matching the configured globs and create a tar.zst
/// archive in the cache directory.
///
/// Returns `Some(archive_filename)` if files were archived, `None` if the
/// output config has no positive globs or no files matched.
fn collect_and_archive_outputs(
    cache_metadata: &CacheMetadata,
    workspace_root: &AbsolutePath,
    cache_dir: &AbsolutePath,
) -> anyhow::Result<Option<Str>> {
    let output_config = &cache_metadata.output_config;

    if output_config.positive_globs.is_empty() {
        return Ok(None);
    }

    let output_files = glob::collect_glob_paths(
        workspace_root,
        &output_config.positive_globs,
        &output_config.negative_globs,
    )?;

    if output_files.is_empty() {
        return Ok(None);
    }

    let archive_name: Str = vite_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
    let archive_path = cache_dir.join(archive_name.as_str());

    archive::create_output_archive(workspace_root, &output_files, &archive_path)?;

    Ok(Some(archive_name))
}

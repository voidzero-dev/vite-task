//! Post-run cache update: decide whether a finished spawn may be cached and,
//! if so, store its fingerprint, captured output, and output archive.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use rustc_hash::FxHashSet;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_plan::cache_metadata::{CacheMetadata, EnvValueHash};
use vite_task_server::Reports;

use super::{
    CacheState,
    fingerprint::{PathRead, PostRunFingerprint, TrackedEnvQuery},
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
    /// Auto-output writes after output exclusions are applied. Empty when
    /// `output_config.includes_auto` is false.
    path_writes: FxHashSet<RelativePathBuf>,
    /// First path that was both read and written during execution, if any.
    /// A non-empty value means caching this task is unsound.
    read_write_overlap: Option<RelativePathBuf>,
}

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
    let CacheState { metadata, globbed_inputs, std_outputs, tracking } = state;
    let fspy = tracking.fspy.as_ref();

    if let Some(reports) = reports
        && reports.cache_disabled
    {
        // A runner-aware tool short-circuited caching via `disableCache()`
        // (e.g. a dev server with no deterministic output).
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::ToolRequested), None);
    }

    // Tool-reported paths to exclude from auto input tracking. Absolute paths
    // are normalized to workspace-relative; anything outside is dropped.
    let ignored_input_rels: FxHashSet<RelativePathBuf> = reports
        .map(|r| normalize_ignored_paths(&r.ignored_inputs, workspace_root))
        .unwrap_or_default();
    let ignored_output_rels: FxHashSet<RelativePathBuf> = reports
        .map(|r| normalize_ignored_paths(&r.ignored_outputs, workspace_root))
        .unwrap_or_default();

    if cancelled {
        // Cancelled (Ctrl-C or sibling failure) — result is untrustworthy.
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::Cancelled), None);
    }

    if !outcome.exit_status.success() {
        // Execution failed with non-zero exit status — don't update cache.
        return (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::NonZeroExitStatus), None);
    }

    let fspy_outcome = observe_fspy(
        outcome,
        metadata,
        fspy,
        &ignored_input_rels,
        &ignored_output_rels,
        workspace_root,
    );

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

    // Collect tool-reported tracked envs for the post-run fingerprint. Env
    // names that the user already declared are skipped because their values
    // are already part of the spawn fingerprint.
    let (tracked_envs, tracked_env_queries) = match collect_tracked_reports(reports, metadata) {
        Ok(tracked_reports) => tracked_reports,
        Err(err) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::PostRunFingerprint(err)),
            );
        }
    };

    // Paths already in globbed_inputs are skipped: the overlap check above
    // guarantees no input modification, so the prerun hash is the correct
    // post-exec hash.
    let empty_path_reads = HashMap::default();
    let path_reads = fspy_outcome.as_ref().map_or(&empty_path_reads, |o| &o.path_reads);
    let post_run_fingerprint = match PostRunFingerprint::create(
        path_reads,
        workspace_root,
        &globbed_inputs,
        tracked_envs,
        tracked_env_queries,
    ) {
        Ok(fingerprint) => fingerprint,
        Err(err) => {
            return (
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::PostRunFingerprint(err)),
            );
        }
    };

    let output_archive = match collect_and_archive_outputs(
        metadata,
        fspy_outcome.as_ref(),
        workspace_root,
        cache_dir,
    ) {
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
/// requested (`tracking.fspy.is_some()`) and compiled in (`cfg(fspy)`). On a
/// `cfg(not(fspy))` build this is always `None`, and [`update_cache`]
/// short-circuits to `FspyUnsupported` when tracking was needed.
///
/// `path_reads` is gated on `input_config.includes_auto`, filtered by
/// user-configured input negatives, and by tool-reported `ignoreInput` paths.
/// `path_writes` is filtered by user-configured output negatives and
/// tool-reported `ignoreOutput` paths before read-write overlap detection.
fn observe_fspy(
    outcome: &ChildOutcome,
    metadata: &CacheMetadata,
    fspy: Option<&super::FspyTracking>,
    ignored_input_rels: &FxHashSet<RelativePathBuf>,
    ignored_output_rels: &FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> Option<TrackingOutcome> {
    #[cfg(fspy)]
    {
        use super::tracked_accesses::TrackedPathAccesses;

        outcome.path_accesses.as_ref().map(|raw| {
            let tracked = TrackedPathAccesses::from_raw(raw, workspace_root);
            let filtered_path_reads: HashMap<RelativePathBuf, PathRead> =
                // fspy can be attached for auto-output-only tasks. In that
                // mode reads must not become inferred inputs.
                if metadata.input_config.includes_auto
                    && let Some(fspy) = fspy
                {
                    tracked
                        .path_reads
                        .iter()
                        .filter(|(path, _)| {
                            !matches_any_glob(path, &fspy.input_negative_globs)
                                && !is_ignored(path, ignored_input_rels)
                        })
                        .map(|(path, read)| (path.clone(), *read))
                        .collect()
                } else {
                    HashMap::default()
                };
            let filtered_path_writes: FxHashSet<RelativePathBuf> =
                // fspy can also be attached for auto-input-only tasks. In that
                // mode writes must not become auto outputs or overlap candidates.
                if metadata.output_config.includes_auto
                    && let Some(fspy) = fspy
                {
                    tracked
                        .path_writes
                        .iter()
                        .filter(|path| {
                            !matches_any_glob(path, &fspy.output_negative_globs)
                                && !is_ignored(path, ignored_output_rels)
                        })
                        .cloned()
                        .collect()
                } else {
                    FxHashSet::default()
                };
            let read_write_overlap =
                filtered_path_reads.keys().find(|p| filtered_path_writes.contains(*p)).cloned();
            TrackingOutcome {
                path_reads: filtered_path_reads,
                path_writes: filtered_path_writes,
                read_write_overlap,
            }
        })
    }
    #[cfg(not(fspy))]
    {
        let _ = (outcome, metadata, fspy, ignored_input_rels, ignored_output_rels, workspace_root);
        None
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

/// Normalize tool-reported absolute paths to cleaned workspace-relative paths.
/// Paths outside the workspace are dropped — they can't contribute to inputs
/// or outputs.
fn normalize_ignored_paths(
    paths: &FxHashSet<Arc<AbsolutePath>>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    paths
        .iter()
        .filter_map(|p| p.strip_prefix(workspace_root).ok().flatten()?.clean().ok())
        .collect()
}

/// Whether `path` is covered by any `ignored` entry. An ignored entry matches
/// itself (exact file) and everything under it (directory subtree).
fn is_ignored(path: &RelativePathBuf, ignored: &FxHashSet<RelativePathBuf>) -> bool {
    if ignored.is_empty() {
        return false;
    }
    ignored.contains(path) || ignored.iter().any(|ig| path.strip_prefix(ig).is_some())
}

fn matches_any_glob(path: &RelativePathBuf, globs: &[wax::Glob<'static>]) -> bool {
    use wax::Program as _;

    globs.iter().any(|glob| glob.is_match(path.as_str()))
}

/// Select tool-reported env records to embed in the post-run fingerprint.
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

/// Select tool-reported bulk env query records to embed in the post-run
/// fingerprint. The full match-set is stored as value hashes.
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
            vite_task_server::EnvQuery::Glob(pattern) => {
                TrackedEnvQuery::Glob(Str::from(pattern.as_ref()))
            }
        };
        tracked_env_queries.insert(query, matches);
    }

    Ok(tracked_env_queries)
}

/// Collect output files and create a tar.zst archive in the cache directory.
///
/// Output files are determined by:
/// - fspy-tracked writes (already empty when `output_config.includes_auto` is false)
/// - Positive output globs (always, if configured)
/// - Negative output globs and tool-reported `ignoreOutput` paths filter
///   fspy-tracked writes before this function receives them
///
/// Returns `Some(archive_filename)` if files were archived, `None` if no output files.
fn collect_and_archive_outputs(
    cache_metadata: &CacheMetadata,
    tracking: Option<&TrackingOutcome>,
    workspace_root: &AbsolutePath,
    cache_dir: &AbsolutePath,
) -> anyhow::Result<Option<Str>> {
    let output_config = &cache_metadata.output_config;

    let mut output_files: FxHashSet<RelativePathBuf> = FxHashSet::default();

    if let Some(t) = tracking {
        output_files.extend(t.path_writes.iter().cloned());
    }

    if !output_config.positive_globs.is_empty() {
        let glob_paths = glob::collect_glob_paths(
            workspace_root,
            &output_config.positive_globs,
            &output_config.negative_globs,
        )?;
        output_files.extend(glob_paths);
    }

    if output_files.is_empty() {
        return Ok(None);
    }

    let mut sorted_files: Vec<RelativePathBuf> = output_files.into_iter().collect();
    sorted_files.sort();

    let archive_name: Str = vite_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
    let archive_path = cache_dir.join(archive_name.as_str());

    archive::create_output_archive(workspace_root, &sorted_files, &archive_path)?;

    Ok(Some(archive_name))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustc_hash::FxHashSet;
    use vite_path::{AbsolutePath, RelativePathBuf};

    use super::normalize_ignored_paths;

    #[test]
    fn normalize_ignored_paths_cleans_relative_components() {
        let workspace_root =
            AbsolutePath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).unwrap();
        let ignored =
            workspace_root.join(if cfg!(windows) { r"pkg\..\cache" } else { "pkg/../cache" });
        let mut ignored_paths = FxHashSet::default();
        ignored_paths.insert(Arc::<AbsolutePath>::from(ignored));

        let normalized = normalize_ignored_paths(&ignored_paths, workspace_root);

        let expected = RelativePathBuf::new("cache").unwrap();
        assert!(normalized.contains(&expected));
    }
}

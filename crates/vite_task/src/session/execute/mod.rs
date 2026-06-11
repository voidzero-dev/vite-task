mod cache_update;
pub mod fingerprint;
pub mod glob;
mod hash;
pub mod pipe;
mod scheduler;
pub mod spawn;
#[cfg(fspy)]
pub mod tracked_accesses;
#[cfg(windows)]
mod win_job;

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use tokio_util::sync::CancellationToken;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::{SpawnExecution, cache_metadata::CacheMetadata};

use self::{
    glob::compute_globbed_inputs,
    pipe::{PipeSinks, StdOutput, pipe_stdio},
    spawn::{ChildHandle, ChildOutcome, SpawnStdio, spawn},
};
use super::{
    cache::{CacheEntryValue, CacheMiss, ExecutionCache, archive},
    event::{
        CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus,
        ExecutionError,
    },
    reporter::{LeafExecutionReporter, PipeWriters, StdioConfig, StdioSuggestion},
};

/// Outcome of a spawned execution.
///
/// Returned by [`execute_spawn`] to communicate what happened. Errors are
/// already reported through `LeafExecutionReporter::finish()` before this
/// value is returned — the caller does not need to handle error display.
pub enum SpawnOutcome {
    /// Cache hit — no process was spawned. Cached outputs were replayed.
    CacheHit,
    /// Process was spawned and exited with this status.
    Spawned(std::process::ExitStatus),
    /// An infrastructure error prevented the process from running
    /// (cache lookup failure or spawn failure).
    /// Already reported through the leaf reporter.
    Failed,
}

/// All valid runtime configurations for a leaf execution, after the cache-hit
/// early-return has been ruled out.
///
/// The type shape enforces two invariants statically:
/// - fspy tracking only exists inside [`ExecutionMode::Cached`] (fspy requires
///   `includes_auto`, which only lives on cache metadata).
/// - Cached execution always owns [`PipeWriters`] (piped stdio is forced so
///   that output can be captured for replay).
enum ExecutionMode<'a> {
    Cached {
        /// Borrowed by [`PipeSinks`] during drain; dropped at end of function.
        pipe_writers: PipeWriters,
        /// Carried through drain into the cache-update phase. Drain writes
        /// into `state.std_outputs` in place via a borrow inside `PipeSinks`.
        state: CacheState<'a>,
    },
    Uncached {
        /// `Some` iff the reporter suggested piped stdio. `None` means the
        /// child inherits stdin/stdout/stderr from the parent; the reporter's
        /// writers were dropped here so we don't hold `std::io::Stdout` while
        /// the child writes to the same FD.
        pipe_writers: Option<PipeWriters>,
    },
}

/// Cached-only state carried from mode construction through the cache-update
/// phase. `std_outputs` starts empty and is written in place during drain via
/// a borrow inside [`PipeSinks::capture`].
struct CacheState<'a> {
    metadata: &'a CacheMetadata,
    globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    /// Captured stdout/stderr for cache replay. Written in place during drain;
    /// always present (possibly empty) once we reach the cache-update phase.
    std_outputs: Vec<StdOutput>,
    /// `Some` iff fspy is enabled (`includes_auto`). Holds the resolved
    /// negative globs used to filter tracked accesses. `None` means fspy
    /// tracking is off for this task.
    fspy_negatives: Option<Vec<wax::Glob<'static>>>,
}

impl<'a> ExecutionMode<'a> {
    /// Fold the cache/fspy/stdio decisions and their associated state into a
    /// single value whose shape encodes the valid combinations. The
    /// uncached-inherited arm drops `stdio_config`'s writers here so we don't
    /// hold `std::io::Stdout` while the child writes to the same FD.
    ///
    /// ─────────────────────────────────────────────────────────────────────
    ///  Before adding a new local variable alongside the mode: think twice.
    ///  Does it make sense for every variant, or only for some?  If it's
    ///  variant-specific (only for `Cached`, only when fspy is on, etc.) put
    ///  it inside the variant (or `CacheState`) so the compiler enforces the
    ///  invariant at construction. Sibling locals drift out of sync with the
    ///  mode and force re-derivation (`if let Some(_) = _`,
    ///  `cache_metadata.is_some_and(_)`) at every downstream use site.
    /// ─────────────────────────────────────────────────────────────────────
    fn build(
        cache_metadata: Option<&'a CacheMetadata>,
        stdio_config: StdioConfig,
        globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    ) -> Result<Self, ExecutionError> {
        let Some(metadata) = cache_metadata else {
            return Ok(Self::Uncached {
                pipe_writers: (stdio_config.suggestion == StdioSuggestion::Piped)
                    .then_some(stdio_config.writers),
            });
        };

        // Resolve input negative globs for fspy path filtering (already
        // workspace-root-relative).
        let fspy_negatives = if metadata.input_config.includes_auto {
            let negatives = metadata
                .input_config
                .negative_globs
                .iter()
                .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(ExecutionError::PostRunFingerprint)?;
            Some(negatives)
        } else {
            None
        };

        Ok(Self::Cached {
            pipe_writers: stdio_config.writers,
            state: CacheState { metadata, globbed_inputs, std_outputs: Vec::new(), fspy_negatives },
        })
    }

    /// The arguments `spawn()` derives from the mode: stdio handling and
    /// whether fspy tracking is on.
    const fn spawn_config(&self) -> (SpawnStdio, bool) {
        match self {
            Self::Cached { state, .. } => (SpawnStdio::Piped, state.fspy_negatives.is_some()),
            Self::Uncached { pipe_writers: Some(_) } => (SpawnStdio::Piped, false),
            Self::Uncached { pipe_writers: None } => (SpawnStdio::Inherited, false),
        }
    }

    /// Borrow the pipe writers (and, when caching, the in-place capture slot)
    /// for the drain. `None` only in the inherited-uncached case, where there
    /// are no pipes to drain.
    fn pipe_sinks(&mut self) -> Option<PipeSinks<'_>> {
        match self {
            Self::Cached { pipe_writers, state } => Some(PipeSinks {
                stdout_writer: &mut pipe_writers.stdout_writer,
                stderr_writer: &mut pipe_writers.stderr_writer,
                capture: Some(&mut state.std_outputs),
            }),
            Self::Uncached { pipe_writers: Some(pipe_writers) } => Some(PipeSinks {
                stdout_writer: &mut pipe_writers.stdout_writer,
                stderr_writer: &mut pipe_writers.stderr_writer,
                capture: None,
            }),
            Self::Uncached { pipe_writers: None } => None,
        }
    }
}

/// Everything the pipeline reports through the single
/// `LeafExecutionReporter::finish()` call at the end of [`execute_spawn`].
///
/// Phases construct a `Report` instead of reporting in place, so finishing —
/// which consumes the boxed reporter — happens in exactly one spot. Each
/// variant carries exactly the data its outcome can be accompanied by:
/// a failure always has an error and never an exit status, a cache hit has
/// neither, and a spawned process always has its exit status. Nonsense
/// pairings are unrepresentable.
enum Report {
    /// An infrastructure error prevented the process from running (or from
    /// being observed to completion).
    Failed { cache_update: CacheUpdateStatus, error: ExecutionError },
    /// Cache hit: captured outputs were replayed, no process ran.
    CacheHit,
    /// The process ran to completion; its exit status is reported regardless
    /// of how the cache update went.
    Spawned {
        exit_status: std::process::ExitStatus,
        cache_update: CacheUpdateStatus,
        error: Option<ExecutionError>,
    },
}

impl Report {
    /// An infrastructure failure outside any cache-specific context.
    const fn failed(error: ExecutionError) -> Self {
        Self::Failed {
            cache_update: CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
            error,
        }
    }

    /// Perform the pipeline's single `finish()` call and yield the outcome
    /// [`execute_spawn`] returns to its caller.
    fn finish(self, reporter: Box<dyn LeafExecutionReporter>) -> SpawnOutcome {
        match self {
            Self::Failed { cache_update, error } => {
                reporter.finish(None, cache_update, Some(error));
                SpawnOutcome::Failed
            }
            Self::CacheHit => {
                reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit),
                    None,
                );
                SpawnOutcome::CacheHit
            }
            Self::Spawned { exit_status, cache_update, error } => {
                reporter.finish(Some(exit_status), cache_update, error);
                SpawnOutcome::Spawned(exit_status)
            }
        }
    }
}

/// Execute a spawned process with cache-aware lifecycle.
///
/// This is a free function (not tied to the scheduler's context) so it can be
/// reused from both graph-based execution and standalone synthetic execution.
///
/// The full lifecycle is:
/// 1. Cache lookup (determines cache status)
/// 2. `leaf_reporter.start(cache_status)` → `StdioConfig`
/// 3. If cache hit: replay cached outputs via `StdioConfig` writers
/// 4. Otherwise: `spawn()` with the chosen stdio mode, drain pipes and wait
///    via [`run_child`], then decide the cache update
///    ([`cache_update::update_cache`])
///
/// Every path reports through the single `finish()` below — errors (cache
/// lookup failure, spawn failure, cache update failure) do not abort the
/// caller.
#[tracing::instrument(level = "debug", skip_all)]
#[expect(
    clippy::too_many_arguments,
    reason = "these are the unavoidable inputs for a free-function cache-aware spawn"
)]
pub async fn execute_spawn(
    mut leaf_reporter: Box<dyn LeafExecutionReporter>,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
    fast_fail_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> SpawnOutcome {
    let pipeline = run(
        leaf_reporter.as_mut(),
        spawn_execution,
        cache,
        workspace_root,
        cache_dir,
        program_name,
        fast_fail_token,
        interrupt_token,
    );
    let report = match pipeline.await {
        Ok(report) | Err(report) => report,
    };
    report.finish(leaf_reporter)
}

/// The spawn pipeline.
///
/// Both sides of the `Result` carry the same [`Report`] on purpose: `Err` is
/// the `?`-short-circuit channel for phases that already determined the final
/// report, `Ok` is the report of a pipeline that ran to the end. The caller
/// unwraps both into the same single `finish()`, so the distinction is pure
/// control flow and a value on either side is equally valid.
#[expect(clippy::too_many_arguments, reason = "forwarded verbatim from `execute_spawn`")]
async fn run(
    reporter: &mut dyn LeafExecutionReporter,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
    fast_fail_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> Result<Report, Report> {
    let cache_metadata = spawn_execution.cache_metadata.as_ref();

    // 1. Determine cache status FIRST by trying cache hit, so the reporter can
    //    display cache status immediately when execution begins. On a lookup
    //    error, `start()` is never called — there is no valid status to show.
    let lookup = lookup_cache(cache_metadata, cache, workspace_root).await?;

    // 2. Report execution start with the looked-up cache status (`start()`
    //    runs exactly once on every arm) and either replay the hit — no need
    //    to execute the command — or carry the globbed inputs into the run.
    let (stdio_config, globbed_inputs) = match lookup {
        CacheLookup::Hit(cached) => {
            let mut stdio_config =
                reporter.start(CacheStatus::Hit { replayed_duration: cached.duration });
            return Ok(replay_cache_hit(
                &mut stdio_config,
                &cached,
                workspace_root,
                cache_dir,
                program_name,
            ));
        }
        CacheLookup::Miss { miss, globbed_inputs } => {
            (reporter.start(CacheStatus::Miss(miss)), globbed_inputs)
        }
        CacheLookup::Disabled => (
            reporter.start(CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata)),
            BTreeMap::new(),
        ),
    };

    // 4. Fold the cache/fspy/stdio decisions into the typed mode.
    let mut mode = ExecutionMode::build(cache_metadata, stdio_config, globbed_inputs)
        .map_err(Report::failed)?;

    // Measure end-to-end duration here — spawn() doesn't track time.
    let start = Instant::now();

    // 5. Spawn. Returns pipes (Piped) or `None` (Inherited) plus a
    //    cancellation-aware wait future.
    let (spawn_stdio, fspy_enabled) = mode.spawn_config();
    let child =
        spawn(&spawn_execution.spawn_command, fspy_enabled, spawn_stdio, fast_fail_token.clone())
            .await
            .map_err(|err| Report::failed(ExecutionError::Spawn(err)))?;

    // 6. Drain the pipes and wait for exit. Box::pin keeps the child-and-pipe
    //    stack off the enclosing future: pipe_stdio alone makes the combined
    //    future large enough to trip clippy::large_futures in every caller
    //    otherwise.
    let outcome = Box::pin(run_child(child, mode.pipe_sinks(), fast_fail_token.clone()))
        .await
        .map_err(|err| Report::failed(ExecutionError::Spawn(err)))?;
    let duration = start.elapsed();

    // 7. Decide the cache update (only when we were in `Cached` mode). Cache
    //    update errors are reported but do not affect the exit status we
    //    return — the process ran, so we return its actual status.
    let cancelled = fast_fail_token.is_cancelled() || interrupt_token.is_cancelled();
    let (cache_update, error) = match mode {
        ExecutionMode::Cached { state, .. } => {
            cache_update::update_cache(
                cache,
                workspace_root,
                cache_dir,
                state,
                &outcome,
                duration,
                cancelled,
            )
            .await
        }
        ExecutionMode::Uncached { .. } => {
            // Caching was disabled for this task.
            (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled), None)
        }
    };

    Ok(Report::Spawned { exit_status: outcome.exit_status, cache_update, error })
}

/// Outcome of the cache-lookup phase. Each variant carries exactly what that
/// outcome provides: a hit owns the cached entry to replay, a miss keeps the
/// reason plus the globbed inputs (reused by the cache-update phase after the
/// run), and disabled has neither.
enum CacheLookup {
    /// Cache hit — the cached entry to replay.
    Hit(CacheEntryValue),
    /// Cache miss — the detailed reason (`NotFound` or `FingerprintMismatch`).
    Miss { miss: CacheMiss, globbed_inputs: BTreeMap<RelativePathBuf, u64> },
    /// Caching is disabled for this task (no cache metadata).
    Disabled,
}

/// Phase 1: compute the globbed inputs and try to hit the cache.
async fn lookup_cache(
    cache_metadata: Option<&CacheMetadata>,
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
) -> Result<CacheLookup, Report> {
    let Some(cache_metadata) = cache_metadata else {
        return Ok(CacheLookup::Disabled);
    };

    // Compute globbed inputs from positive globs at execution time.
    // Globs are already workspace-root-relative (resolved at task graph stage).
    let globbed_inputs = compute_globbed_inputs(
        workspace_root,
        &cache_metadata.input_config.positive_globs,
        &cache_metadata.input_config.negative_globs,
    )
    .map_err(|err| {
        Report::failed(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err })
    })?;

    match cache.try_hit(cache_metadata, &globbed_inputs, workspace_root).await {
        Ok(Ok(cached)) => Ok(CacheLookup::Hit(cached)),
        Ok(Err(miss)) => Ok(CacheLookup::Miss { miss, globbed_inputs }),
        Err(err) => {
            Err(Report::failed(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }))
        }
    }
}

/// Phase 3 (cache hit): replay the captured stdout/stderr and restore the
/// output archive.
fn replay_cache_hit(
    stdio_config: &mut StdioConfig,
    cached: &CacheEntryValue,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
) -> Report {
    for output in cached.std_outputs.iter() {
        let writer: &mut dyn std::io::Write = match output.kind {
            pipe::OutputKind::StdOut => &mut stdio_config.writers.stdout_writer,
            pipe::OutputKind::StdErr => &mut stdio_config.writers.stderr_writer,
        };
        let _ = writer.write_all(&output.content);
        let _ = writer.flush();
    }

    // Restore output files from the cached archive. Failure here means the
    // archive file is missing, truncated, or otherwise unreadable — the
    // task can't proceed because the cache promised the outputs would be
    // restored. Surface a recovery instruction rather than just the raw
    // I/O error so users know to clear the cache.
    if let Some(ref archive_name) = cached.output_archive {
        let archive_path = cache_dir.join(archive_name.as_str());
        if let Err(err) = archive::extract_output_archive(workspace_root, &archive_path) {
            let err = err.context(vite_str::format!(
                "failed to restore cached outputs from {}; the archive may have been deleted \
                 or corrupted. Run `{program_name} cache clean` to clear the cache.",
                archive_path.as_path().display()
            ));
            return Report::Failed {
                cache_update: CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit),
                error: ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err },
            };
        }
    }

    Report::CacheHit
}

/// Phase 6: drain the child's pipes (if piped) and wait for exit, with a
/// single error sink — a pipe failure cancels (so the wait kills the child
/// instead of orphaning it) and surfaces through the same returned result as
/// a wait failure.
async fn run_child(
    mut child: ChildHandle,
    sinks: Option<PipeSinks<'_>>,
    fast_fail_token: CancellationToken,
) -> anyhow::Result<ChildOutcome> {
    let pipe_result: anyhow::Result<()> = if let Some(sinks) = sinks {
        let stdout = child.stdout.take().expect("SpawnStdio::Piped yields a stdout pipe");
        let stderr = child.stderr.take().expect("SpawnStdio::Piped yields a stderr pipe");
        #[expect(
            clippy::large_futures,
            reason = "pipe_stdio streams child I/O and creates a large future"
        )]
        let r = pipe_stdio(stdout, stderr, sinks, fast_fail_token.clone()).await;
        r.map_err(anyhow::Error::from)
    } else {
        Ok(())
    };

    match pipe_result {
        Ok(()) => child.wait.await.map_err(anyhow::Error::from),
        Err(err) => {
            // Pipe failed — cancel so `child.wait` kills the child instead of
            // orphaning it.
            fast_fail_token.cancel();
            let _ = child.wait.await;
            Err(err)
        }
    }
}

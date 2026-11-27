mod cache_update;
pub mod fingerprint;
pub mod glob;
mod hash;
pub mod ignored_paths;
pub mod pipe;
mod rebuildable;
mod redact;
mod scheduler;
pub mod spawn;
#[cfg(fspy)]
pub mod tracked_accesses;
#[cfg(windows)]
mod win_job;

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    num::NonZeroI32,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::future::LocalBoxFuture;
use tokio_util::sync::CancellationToken;
use vt_glob::path::PathGlobSet;
use vt_ipc_shared::NODE_CLIENT_PATH_ENV_NAME;
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_plan::{SpawnExecution, cache_metadata::CacheMetadata};
use vt_server::{Recorder, Reports, ServerHandle, StopAccepting, serve};

use self::{
    glob::compute_globbed_inputs,
    pipe::{PipeSinks, StdOutput, pipe_stdio},
    spawn::{ChildOutcome, ChildWait, PipedChild, PipedMode, spawn_inherited, spawn_piped},
};
use super::{
    cache::{
        BaselineDetails, CacheHit, CacheHitSource, CacheMiss, ExecutionCache, FileCheck,
        run_off_thread,
    },
    event::{
        CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason, CacheStatus,
        CacheUpdateOutcome, ExecutionError, LeafOutcome, LiveSpawnCacheStatus, RunKind,
        SpawnLeafOutcome, non_zero_exit_code,
    },
    reporter::{LeafExecutionReporter, PipeWriters, StdioConfig, StdioSuggestion},
};

/// Outcome of a spawned execution.
///
/// Returned by [`execute_spawn`] to communicate what happened. Errors are
/// already reported through `LeafExecutionReporter::finish()` before this
/// value is returned — the caller does not need to handle error display.
pub enum SpawnOutcome {
    /// Cache hit — no process was spawned. Cached outputs were replayed, except
    /// in a dry run.
    CacheHit,
    /// In a dry run, the cache missed or is disabled, so a real run would
    /// spawn the process. Nothing was spawned.
    WouldSpawn,
    /// Process was spawned and exited with status 0.
    SpawnedSuccess,
    /// Process was spawned and exited with a non-zero exit code.
    SpawnedFailed(NonZeroI32),
    /// An infrastructure error prevented the process from running, prevented
    /// observing it to completion, or prevented restoring a cache hit's outputs
    /// (cache lookup, cache restore, IPC bind, spawn, output forwarding, or
    /// wait failure). Already reported through the leaf reporter.
    InfraError,
    /// The run was cancelled during the cache lookup, so the process didn't
    /// start and no cached outputs were restored. Like a task that was never
    /// scheduled, it isn't reported.
    Cancelled,
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
    /// Runner-aware tracking for cached tasks: an IPC server is always
    /// available, and fspy path tracing is attached only when auto input or
    /// output inference needs it. Parts are borrowed in place during the
    /// wait/join; the struct is never moved out.
    tracking: Tracking<'a>,
}

/// The IPC server's driver future: resolves with the recorded reports after
/// [`StopAccepting::signal`] fires and all in-flight clients drain.
type IpcDriver = LocalBoxFuture<'static, Result<Recorder, vt_server::Error>>;

/// fspy path-tracking state, present only when a cached task needs automatic
/// input or output inference.
#[cfg_attr(not(fspy), expect(dead_code, reason = "the globs only filter fspy's accesses"))]
struct FspyTracking<'a> {
    input_negative_globs: PathGlobSet<'a>,
    output_negative_globs: PathGlobSet<'a>,
}

/// Per-task runner-aware tracking: IPC server handle plus optional fspy state.
/// Lifetime-tied to a single `execute_spawn` call.
struct Tracking<'a> {
    fspy: Option<FspyTracking<'a>>,
    ipc_envs: Vec<(&'static OsStr, OsString)>,
    ipc_server_fut: IpcDriver,
    stop_accepting: StopAccepting,
}

impl Tracking<'_> {
    /// The extra envs to inject into the child: IPC connection info + the
    /// napi addon path runner-aware tools `require()`.
    fn injected_envs(&self) -> Vec<(&OsStr, &OsStr)> {
        let mut envs: Vec<(&OsStr, &OsStr)> =
            self.ipc_envs.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
        envs.push((
            OsStr::new(NODE_CLIENT_PATH_ENV_NAME),
            crate::napi_client::napi_client_path().as_path().as_os_str(),
        ));
        envs
    }
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

        let fspy = if metadata.input_config.includes_auto || metadata.output_config.includes_auto {
            // Resolve negative globs for fspy path filtering (already
            // workspace-root-relative).
            let input_negative_globs = PathGlobSet::new(&metadata.input_config.negative_globs)
                .map_err(|err| ExecutionError::PostRunFingerprint(err.into()))?;
            let output_negative_globs = PathGlobSet::new(&metadata.output_config.negative_globs)
                .map_err(|err| ExecutionError::PostRunFingerprint(err.into()))?;
            Some(FspyTracking { input_negative_globs, output_negative_globs })
        } else {
            None
        };

        // Bind runner IPC for every cached task. The merged cache-control API
        // (`disableCache`) must work even when a task uses explicit inputs and
        // therefore does not need fspy auto-input inference.
        let (ipc_envs, ServerHandle { driver, stop_accepting }) =
            serve(Recorder::new(Arc::clone(&metadata.unfiltered_envs)))
                .map_err(ExecutionError::IpcServerBind)?;
        let tracking =
            Tracking { fspy, ipc_envs: ipc_envs.collect(), ipc_server_fut: driver, stop_accepting };

        Ok(Self::Cached {
            pipe_writers: stdio_config.writers,
            state: CacheState { metadata, globbed_inputs, std_outputs: Vec::new(), tracking },
        })
    }

    /// Phases 5 & 6: spawn the child process, drain its pipes (if piped), and
    /// wait for exit — concurrently with the IPC driver when cached.
    ///
    /// Consumes `self` and returns a mode-typed [`RunOutcome`] so downstream
    /// phases cannot pair an uncached run with IPC reports (or a cached run
    /// without them).
    async fn spawn_and_run(
        self,
        spawn_command: &vt_plan::SpawnCommand,
        start: Instant,
        fast_fail_token: &CancellationToken,
    ) -> Result<RunOutcome<'a>, ExecutionError> {
        match self {
            Self::Cached { mut pipe_writers, mut state } => {
                let piped_mode = if state.tracking.fspy.is_some() {
                    PipedMode::Tracked
                } else {
                    PipedMode::Plain
                };
                let child = spawn_piped(
                    spawn_command,
                    piped_mode,
                    fast_fail_token.clone(),
                    state.tracking.injected_envs(),
                )
                .await
                .map_err(ExecutionError::Spawn)?;
                tracing::debug!(
                    fspy = (piped_mode == PipedMode::Tracked),
                    elapsed = ?start.elapsed(),
                    "spawned the task process"
                );

                // The IPC driver must be polled during pipe drain — otherwise a
                // tool doing a blocking `getEnv` can deadlock: child stalls on
                // IPC, stdout stays open, pipe_stdio waits for EOF, driver
                // never runs. `stop_accepting` fires right after child exit so
                // the driver drains any in-flight clients.
                //
                // Box::pin keeps the child-and-pipe stack off the enclosing
                // future: pipe_stdio alone makes the combined future large
                // enough to trip clippy::large_futures in every caller.
                let sinks = PipeSinks {
                    stdout_writer: &mut pipe_writers.stdout_writer,
                    stderr_writer: &mut pipe_writers.stderr_writer,
                    capture: Some(&mut state.std_outputs),
                };
                let stop_accepting = &state.tracking.stop_accepting;
                let child_work = async {
                    let result =
                        Box::pin(run_piped_child(child, sinks, fast_fail_token.clone())).await;
                    stop_accepting.signal();
                    result
                };
                let (wait_result, join_result) =
                    tokio::join!(child_work, &mut state.tracking.ipc_server_fut);
                if let Err(e) = &join_result {
                    tracing::warn!(?e, "IPC server failed; cache will not be updated");
                }
                Ok(RunOutcome::Cached {
                    state: Box::new(state),
                    outcome: wait_result?,
                    ipc_result: join_result.map(Recorder::into_reports),
                })
            }
            Self::Uncached { pipe_writers: Some(mut pipe_writers) } => {
                let child = spawn_piped(
                    spawn_command,
                    PipedMode::Plain,
                    fast_fail_token.clone(),
                    std::iter::empty::<(&OsStr, &OsStr)>(),
                )
                .await
                .map_err(ExecutionError::Spawn)?;
                tracing::debug!(
                    fspy = false,
                    elapsed = ?start.elapsed(),
                    "spawned the task process"
                );

                let sinks = PipeSinks {
                    stdout_writer: &mut pipe_writers.stdout_writer,
                    stderr_writer: &mut pipe_writers.stderr_writer,
                    capture: None,
                };
                let child_work = Box::pin(run_piped_child(child, sinks, fast_fail_token.clone()));
                Ok(RunOutcome::Uncached { outcome: child_work.await? })
            }
            Self::Uncached { pipe_writers: None } => {
                let child = spawn_inherited(spawn_command, fast_fail_token.clone())
                    .map_err(ExecutionError::Spawn)?;
                tracing::debug!(
                    fspy = false,
                    elapsed = ?start.elapsed(),
                    "spawned the task process"
                );

                // Measure post-spawn runtime for the exit debug log, matching
                // `run_piped_child` rather than including spawn overhead.
                Ok(RunOutcome::Uncached { outcome: wait_child(child.wait, Instant::now()).await? })
            }
        }
    }
}

/// Mode-typed result of [`ExecutionMode::spawn_and_run`].
///
/// Couples `CacheState` and the IPC server's reports inside [`Self::Cached`] so
/// cached-without-IPC and uncached-with-IPC states are unrepresentable.
enum RunOutcome<'a> {
    Cached {
        state: Box<CacheState<'a>>,
        outcome: ChildOutcome,
        ipc_result: Result<Reports, vt_server::Error>,
    },
    Uncached {
        outcome: ChildOutcome,
    },
}

impl RunOutcome<'_> {
    const fn exit_status(&self) -> std::process::ExitStatus {
        match self {
            Self::Cached { outcome, .. } | Self::Uncached { outcome } => outcome.exit_status,
        }
    }
}

/// Everything the spawn pipeline reports through the single
/// `LeafExecutionReporter::finish()` call at the end of [`execute_spawn`].
///
/// Separate from [`LeafOutcome`] so that in-process execution is unrepresentable
/// inside the spawn pipeline.
enum SpawnReport {
    CacheHit {
        replayed_duration: Duration,
        source: CacheHitSource,
        baseline: Option<Arc<BaselineDetails>>,
    },
    RestoreFailed {
        source: CacheHitSource,
        error: ExecutionError,
    },
    Spawned {
        cache_status: LiveSpawnCacheStatus,
        outcome: SpawnLeafOutcome,
    },
    FailedBeforeStart(ExecutionError),
    Cancelled,
}

impl SpawnReport {
    /// Perform the pipeline's single `finish()` call and yield the
    /// [`SpawnOutcome`] [`execute_spawn`] returns to its caller.
    fn finish(self, reporter: Box<dyn LeafExecutionReporter>) -> SpawnOutcome {
        let (spawn_outcome, leaf_outcome) = match self {
            Self::CacheHit { replayed_duration, source, baseline } => (
                SpawnOutcome::CacheHit,
                LeafOutcome::CacheHit { replayed_duration, source, baseline },
            ),
            Self::RestoreFailed { source, error } => {
                (SpawnOutcome::InfraError, LeafOutcome::RestoreFailed { source, error })
            }
            Self::Spawned { cache_status, outcome } => {
                let spawn_outcome = match &outcome {
                    SpawnLeafOutcome::Success { .. } => SpawnOutcome::SpawnedSuccess,
                    SpawnLeafOutcome::Failed { exit_code } => {
                        SpawnOutcome::SpawnedFailed(*exit_code)
                    }
                    SpawnLeafOutcome::SpawnError(_) => SpawnOutcome::InfraError,
                    SpawnLeafOutcome::WouldSpawn => SpawnOutcome::WouldSpawn,
                };
                (spawn_outcome, LeafOutcome::Spawned { cache_status, outcome })
            }
            Self::FailedBeforeStart(error) => {
                (SpawnOutcome::InfraError, LeafOutcome::FailedBeforeStart(error))
            }
            Self::Cancelled => (SpawnOutcome::Cancelled, LeafOutcome::Cancelled),
        };
        reporter.finish(leaf_outcome);
        spawn_outcome
    }
}

/// Execute a spawned process with cache-aware lifecycle.
///
/// This is a free function (not tied to the scheduler's context) so it can be
/// reused from both graph-based execution and standalone synthetic execution.
///
/// The full lifecycle is:
/// 1. Cache lookup (determines cache status). If the run was cancelled by
///    the time it ends, nothing else happens.
/// 2. `leaf_reporter.start(&cache_status)` → `StdioConfig`
/// 3. If cache hit: replay cached outputs via `StdioConfig` writers
/// 4. Otherwise: spawn the child with the chosen stdio/tracking mode, drain
///    pipes (if piped) and wait via [`ExecutionMode::spawn_and_run`], then
///    decide the cache update ([`cache_update::update_cache`])
///
/// A dry run (`RunKind::Dry`) only looks: the lookup in step 1 writes
/// nothing, and steps 3 and 4 are skipped.
///
/// `file_check` says how many changed files a cache miss lists.
///
/// Cancelling `fast_fail_token` kills the process. `cancel_token` must be a
/// child of `fast_fail_token` that Ctrl-C also cancels: cancelling it stops
/// remote cache lookups and prevents caching. A cached run's upload to the
/// remote cache runs in the background, and the caller waits for it with
/// [`ExecutionCache::wait_for_uploads`].
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
    run_kind: RunKind,
    file_check: FileCheck,
    fast_fail_token: CancellationToken,
    cancel_token: CancellationToken,
) -> SpawnOutcome {
    let pipeline = run(
        leaf_reporter.as_mut(),
        spawn_execution,
        cache,
        workspace_root,
        cache_dir,
        program_name,
        run_kind,
        file_check,
        fast_fail_token,
        cancel_token,
    );
    let report = match pipeline.await {
        Ok(report) | Err(report) => report,
    };
    report.finish(leaf_reporter)
}

/// The spawn pipeline.
///
/// Both sides of the `Result` carry the same [`SpawnReport`] on purpose: `Err`
/// is the `?`-short-circuit channel for phases that already determined the
/// final outcome, `Ok` is the outcome of a pipeline that ran to the end. The
/// caller unwraps both into the same single `finish()`, so the distinction is
/// pure control flow and a value on either side is equally valid.
#[expect(clippy::too_many_arguments, reason = "forwarded verbatim from `execute_spawn`")]
#[expect(clippy::too_many_lines, reason = "the phases of one spawn, in order")]
async fn run(
    reporter: &mut dyn LeafExecutionReporter,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
    run_kind: RunKind,
    file_check: FileCheck,
    fast_fail_token: CancellationToken,
    cancel_token: CancellationToken,
) -> Result<SpawnReport, SpawnReport> {
    let cache_metadata = spawn_execution.cache_metadata.as_ref();

    // 1. Determine cache status FIRST by trying cache hit, so the reporter can
    //    display cache status immediately when execution begins. On a lookup
    //    error, `start()` is never called — there is no valid status to show.
    //    A lookup can take a while, especially a remote one. If the run was
    //    cancelled meanwhile, neither start the process nor restore outputs.
    let lookup = lookup_cache(
        cache_metadata,
        cache,
        workspace_root,
        cache_dir,
        run_kind,
        file_check,
        &cancel_token,
    )
    .await?;
    if cancel_token.is_cancelled() {
        return Err(SpawnReport::Cancelled);
    }

    // 2. Report execution start with the looked-up cache status (`start()`
    //    runs exactly once on every arm) and either replay the hit — no need
    //    to execute the command — or carry the globbed inputs into the run.
    let (stdio_config, spawn_cache_status, globbed_inputs) = match lookup {
        CacheLookup::Hit { hit, metadata } => {
            let mut stdio_config = reporter.start(&CacheStatus::Hit {
                replayed_duration: hit.value.duration,
                source: hit.source,
                restores_outputs: hit.value.output_archive.is_some(),
                baseline: hit.baseline.clone(),
            });
            if run_kind == RunKind::Dry {
                return Ok(SpawnReport::CacheHit {
                    replayed_duration: hit.value.duration,
                    source: hit.source,
                    baseline: hit.baseline,
                });
            }
            return Ok(replay_cache_hit(
                &mut stdio_config,
                hit,
                cache,
                metadata,
                workspace_root,
                cache_dir,
                program_name,
            )
            .await);
        }
        CacheLookup::Miss { miss, globbed_inputs } => {
            let stdio_config = reporter.start(&CacheStatus::Miss(miss.clone()));
            (stdio_config, LiveSpawnCacheStatus::Miss(miss), globbed_inputs)
        }
        CacheLookup::Disabled => {
            let stdio_config =
                reporter.start(&CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata));
            (stdio_config, LiveSpawnCacheStatus::Disabled, BTreeMap::new())
        }
    };
    if run_kind == RunKind::Dry {
        return Ok(SpawnReport::Spawned {
            cache_status: spawn_cache_status,
            outcome: SpawnLeafOutcome::WouldSpawn,
        });
    }

    let spawn_err = |cache_status, err| SpawnReport::Spawned {
        cache_status,
        outcome: SpawnLeafOutcome::SpawnError(err),
    };

    // 4. Fold the cache/fspy/stdio decisions into the typed mode.
    let mode = match ExecutionMode::build(cache_metadata, stdio_config, globbed_inputs) {
        Ok(mode) => mode,
        Err(err) => return Err(spawn_err(spawn_cache_status, err)),
    };

    // Measure end-to-end duration here — spawn doesn't track time.
    let start = Instant::now();

    // 5 & 6. Spawn the child, drain pipes (if piped), and wait for exit
    //        (concurrently with the IPC driver when cached).
    let run_outcome =
        match mode.spawn_and_run(&spawn_execution.spawn_command, start, &fast_fail_token).await {
            Ok(result) => result,
            Err(err) => return Err(spawn_err(spawn_cache_status, err)),
        };
    let duration = start.elapsed();

    // Non-zero exit: cache is never updated.
    let exit_status = run_outcome.exit_status();
    if !exit_status.success() {
        return Ok(SpawnReport::Spawned {
            cache_status: spawn_cache_status,
            outcome: SpawnLeafOutcome::Failed { exit_code: non_zero_exit_code(exit_status) },
        });
    }

    // 7. Decide the cache update (only when we were in `Cached` mode). Cache
    //    update errors do not affect `SpawnOutcome` or fast-fail (the process
    //    exited 0, so we report `SpawnedSuccess`), though the summary reporter
    //    still turns post-run cache errors into a non-zero run exit code.
    let cache_update = match run_outcome {
        RunOutcome::Cached { state, outcome, ipc_result } => {
            #[cfg(not(fspy))]
            let _ = outcome;
            // Extract reports, or short-circuit when the IPC server failed. An
            // Err here means reports may be incomplete: caching this run would
            // risk stale inputs/outputs, so skip all cache-related computation.
            let reports = match ipc_result {
                Ok(reports) => {
                    tracing::debug!(?reports, "runner-aware tools reported");
                    reports
                }
                Err(err) => {
                    return Ok(SpawnReport::Spawned {
                        cache_status: spawn_cache_status,
                        outcome: SpawnLeafOutcome::Success {
                            cache_update: CacheUpdateOutcome::NotUpdated(
                                CacheNotUpdatedReason::IpcServerError(err),
                            ),
                        },
                    });
                }
            };
            cache_update::update_cache(
                cache,
                workspace_root,
                cache_dir,
                *state,
                #[cfg(fspy)]
                outcome.path_accesses.as_ref(),
                Some(&reports),
                duration,
                &cancel_token,
            )
            .await
        }
        RunOutcome::Uncached { .. } => {
            // Caching was disabled for this task.
            CacheUpdateOutcome::NotUpdated(CacheNotUpdatedReason::CacheDisabled)
        }
    };

    Ok(SpawnReport::Spawned {
        cache_status: spawn_cache_status,
        outcome: SpawnLeafOutcome::Success { cache_update },
    })
}

/// Outcome of the cache-lookup phase. Each variant carries exactly what that
/// outcome provides: a hit owns the cached entry to replay, a miss keeps the
/// reason plus the globbed inputs (reused by the cache-update phase after the
/// run), and disabled has neither.
enum CacheLookup<'a> {
    /// Cache hit — the cached entry to replay, where it came from, and the
    /// metadata that looked it up.
    Hit { hit: CacheHit, metadata: &'a CacheMetadata },
    /// Cache miss — the detailed reason (`NotFound`, `Changed`, or a failed
    /// remote read).
    Miss { miss: CacheMiss, globbed_inputs: BTreeMap<RelativePathBuf, u64> },
    /// Caching is disabled for this task (no cache metadata).
    Disabled,
}

/// Phase 1: compute the globbed inputs and try to hit the cache. A remote hit
/// downloads its output archive into `cache_dir`, except in a dry run. The
/// inputs are hashed, and entries checked, off the scheduler's thread (see
/// [`run_off_thread`]). The lookup stops when `cancel_token` is cancelled, and
/// is then [`SpawnReport::Cancelled`] or a miss that isn't reported.
async fn lookup_cache<'a>(
    cache_metadata: Option<&'a CacheMetadata>,
    cache: &ExecutionCache,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    run_kind: RunKind,
    file_check: FileCheck,
    cancel_token: &CancellationToken,
) -> Result<CacheLookup<'a>, SpawnReport> {
    let Some(cache_metadata) = cache_metadata else {
        return Ok(CacheLookup::Disabled);
    };

    // Compute globbed inputs from positive globs at execution time.
    // Globs are already workspace-root-relative (resolved at task graph stage).
    let (root, input_config) = (Arc::clone(workspace_root), cache_metadata.input_config.clone());
    let hashed = run_off_thread(cancel_token, move || {
        let start = Instant::now();
        let globbed_inputs = compute_globbed_inputs(
            &root,
            &input_config.positive_globs,
            &input_config.negative_globs,
        )?;
        tracing::debug!(
            files = globbed_inputs.len(),
            elapsed = ?start.elapsed(),
            "hashed the globbed inputs"
        );
        anyhow::Ok(globbed_inputs)
    })
    .await
    .ok_or(SpawnReport::Cancelled)?;
    let globbed_inputs = Arc::new(hashed.map_err(|err| {
        SpawnReport::FailedBeforeStart(ExecutionError::Cache {
            kind: CacheErrorKind::Lookup,
            source: err,
        })
    })?);

    match cache
        .try_hit(
            cache_metadata,
            &globbed_inputs,
            workspace_root,
            cache_dir,
            run_kind,
            file_check,
            cancel_token,
        )
        .await
    {
        Ok(Some(Ok(hit))) => Ok(CacheLookup::Hit { hit, metadata: cache_metadata }),
        // Unless the lookup was cancelled, its checks have finished and
        // dropped their references, so the inputs aren't copied.
        Ok(Some(Err(miss))) => {
            Ok(CacheLookup::Miss { miss, globbed_inputs: Arc::unwrap_or_clone(globbed_inputs) })
        }
        Ok(None) => Err(SpawnReport::Cancelled),
        Err(err) => Err(SpawnReport::FailedBeforeStart(ExecutionError::Cache {
            kind: CacheErrorKind::Lookup,
            source: err,
        })),
    }
}

/// Phase 3 (cache hit): replay the captured stdout/stderr and restore the
/// output files.
async fn replay_cache_hit(
    stdio_config: &mut StdioConfig,
    hit: CacheHit,
    cache: &ExecutionCache,
    cache_metadata: &CacheMetadata,
    workspace_root: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
) -> SpawnReport {
    for output in hit.value.std_outputs.iter() {
        let writer: &mut dyn std::io::Write = match output.kind {
            pipe::OutputKind::StdOut => &mut stdio_config.writers.stdout_writer,
            pipe::OutputKind::StdErr => &mut stdio_config.writers.stderr_writer,
        };
        let _ = writer.write_all(&output.content);
        let _ = writer.flush();
    }

    // Failure here means the archive is missing or unreadable, or its files
    // can't be written. The task fails because the cache promised the
    // outputs would be restored.
    if let Err(err) = cache.restore(cache_metadata, &hit, workspace_root, cache_dir).await {
        let error = match hit.source {
            CacheHitSource::Local => {
                ExecutionError::LocalCacheRestore { program_name: program_name.into(), source: err }
            }
            // Not recorded locally, so there's no entry to clear.
            CacheHitSource::Remote => {
                ExecutionError::Cache { kind: CacheErrorKind::Restore, source: err }
            }
        };
        return SpawnReport::RestoreFailed { source: hit.source, error };
    }

    SpawnReport::CacheHit {
        replayed_duration: hit.value.duration,
        source: hit.source,
        baseline: hit.baseline,
    }
}

/// Wait for an already-spawned child to exit and log its exit status.
async fn wait_child(wait: ChildWait, start: Instant) -> Result<ChildOutcome, ExecutionError> {
    let wait_result = wait.await.map_err(|err| ExecutionError::WaitForTaskProcessExit(err.into()));
    if let Ok(outcome) = &wait_result {
        tracing::debug!(
            success = outcome.exit_status.success(),
            exit_code = outcome.exit_status.code(),
            elapsed = ?start.elapsed(),
            "the task process exited"
        );
    }
    wait_result
}

/// Phase 6 (piped): drain the child's stdout/stderr pipes and wait for exit.
/// A pipe failure cancels the child so the wait future kills it instead of
/// orphaning it.
async fn run_piped_child(
    child: PipedChild,
    sinks: PipeSinks<'_>,
    fast_fail_token: CancellationToken,
) -> Result<ChildOutcome, ExecutionError> {
    let start = Instant::now();
    #[expect(
        clippy::large_futures,
        reason = "pipe_stdio streams child I/O and creates a large future"
    )]
    let pipe_result =
        pipe_stdio(child.pipes.stdout, child.pipes.stderr, sinks, fast_fail_token.clone())
            .await
            .map_err(|err| ExecutionError::ForwardTaskProcessOutput(err.into()));

    match pipe_result {
        Ok(()) => wait_child(child.wait, start).await,
        Err(err) => {
            // Pipe failed — cancel so `child.wait` kills the child instead of
            // orphaning it.
            fast_fail_token.cancel();
            let _ = child.wait.await;
            Err(err)
        }
    }
}

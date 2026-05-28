pub mod fingerprint;
pub mod glob;
mod hash;
pub mod pipe;
pub mod spawn;
#[cfg(fspy)]
pub mod tracked_accesses;
#[cfg(windows)]
mod win_job;

use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::Write as _,
    sync::Arc,
    time::Instant,
};

use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, SpawnExecution,
    cache_metadata::CacheMetadata, execution_graph::ExecutionNodeIndex,
};
use vite_task_server::{Recorder, Reports, StopAccepting};
use wax::Program as _;

#[cfg(fspy)]
use self::tracked_accesses::TrackedPathAccesses;
use self::{
    fingerprint::{PathRead, PostRunFingerprint},
    glob::compute_globbed_inputs,
    pipe::{PipeSinks, StdOutput, pipe_stdio},
    spawn::{SpawnStdio, spawn},
};
use super::{
    cache::{CacheEntryValue, ExecutionCache, archive},
    event::{
        CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus,
        ExecutionError,
    },
    reporter::{
        ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
        PipeWriters, StdioSuggestion,
    },
};
use crate::{Session, collections::HashMap};

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

/// Holds shared references needed during graph execution.
///
/// The `reporter` field is wrapped in `RefCell` because concurrent futures
/// (via `FuturesUnordered`) need shared access to create leaf reporters.
/// Since all futures run on a single thread (no `tokio::spawn`), `RefCell`
/// is sufficient for interior mutability.
///
/// Cache fields are passed through to [`execute_spawn`] for cache-aware execution.
struct ExecutionContext<'a> {
    /// The graph-level reporter, used to create leaf reporters via `new_leaf_execution()`.
    /// Wrapped in `RefCell` for shared access from concurrent task futures.
    reporter: &'a RefCell<Box<dyn GraphExecutionReporter>>,
    /// The execution cache for looking up and storing cached results.
    cache: &'a ExecutionCache,
    /// Base path for resolving relative paths in cache entries.
    /// Typically the workspace root.
    cache_base_path: &'a Arc<AbsolutePath>,
    /// Directory where cache files (db, archives) are stored.
    cache_dir: &'a AbsolutePath,
    /// Public-facing program name (e.g. `vp`), used in user-facing error
    /// messages that suggest a CLI command (e.g. `cache clean`).
    program_name: &'a str,
    /// Token cancelled when a task fails. Kills in-flight child processes
    /// (via `start_kill` in spawn.rs), prevents scheduling new tasks, and
    /// prevents caching results of concurrently-running tasks.
    fast_fail_token: CancellationToken,
    /// Token cancelled by Ctrl-C. Unlike `fast_fail_token` (which kills
    /// children), this only prevents scheduling new tasks and caching
    /// results — running processes are left to handle SIGINT naturally.
    interrupt_token: CancellationToken,
}

impl ExecutionContext<'_> {
    /// Returns true if execution has been cancelled, either by a task
    /// failure (fast-fail) or by Ctrl-C (interrupt).
    fn cancelled(&self) -> bool {
        self.fast_fail_token.is_cancelled() || self.interrupt_token.is_cancelled()
    }

    /// Execute all tasks in an execution graph concurrently, respecting dependencies.
    ///
    /// Uses a DAG scheduler: tasks whose dependencies have all completed are scheduled
    /// onto a `FuturesUnordered`, bounded by a per-graph `Semaphore` with
    /// `concurrency_limit` permits. Each recursive `Expanded` graph creates its own
    /// semaphore, so nested graphs have independent concurrency limits.
    ///
    /// Fast-fail: if any task fails, `execute_leaf` cancels the `fast_fail_token`
    /// (killing in-flight child processes). Ctrl-C cancels the `interrupt_token`.
    /// Either cancellation causes this method to close the semaphore, drain
    /// remaining futures, and return.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_expanded_graph(&self, graph: &ExecutionGraph) {
        if graph.graph.node_count() == 0 {
            return;
        }

        let semaphore =
            Arc::new(Semaphore::new(graph.concurrency_limit.min(Semaphore::MAX_PERMITS)));

        // Compute dependency count for each node.
        // Edge A→B means "A depends on B", so A's dependency count = outgoing edge count.
        let mut dep_count: FxHashMap<ExecutionNodeIndex, usize> = FxHashMap::default();
        for node_ix in graph.graph.node_indices() {
            dep_count.insert(node_ix, graph.graph.neighbors(node_ix).count());
        }

        let mut futures = FuturesUnordered::new();

        // Schedule initially ready nodes (no dependencies).
        for (&node_ix, &count) in &dep_count {
            if count == 0 {
                futures.push(self.spawn_node(graph, node_ix, &semaphore));
            }
        }

        // Process completions and schedule newly ready dependents.
        // On failure, `execute_leaf` cancels the token — we detect it here, close
        // the semaphore (so pending acquires fail immediately), and drain.
        while let Some(completed_ix) = futures.next().await {
            if self.cancelled() {
                semaphore.close();
                while futures.next().await.is_some() {}
                return;
            }

            // Find dependents of the completed node (nodes that depend on it).
            // Edge X→completed means "X depends on completed", so X is a predecessor
            // in graph direction = neighbor in Incoming direction.
            for dependent in graph.graph.neighbors_directed(completed_ix, Direction::Incoming) {
                let count = dep_count.get_mut(&dependent).expect("all nodes are in dep_count");
                *count -= 1;
                if *count == 0 {
                    futures.push(self.spawn_node(graph, dependent, &semaphore));
                }
            }
        }
    }

    /// Create a future that acquires a semaphore permit, then executes a graph node.
    ///
    /// On failure, `execute_node` cancels the `fast_fail_token` — the caller
    /// detects this after the future completes. On semaphore closure or prior
    /// cancellation, the node is skipped.
    fn spawn_node<'a>(
        &'a self,
        graph: &'a ExecutionGraph,
        node_ix: ExecutionNodeIndex,
        semaphore: &Arc<Semaphore>,
    ) -> LocalBoxFuture<'a, ExecutionNodeIndex> {
        let sem = semaphore.clone();
        async move {
            if let Ok(_permit) = sem.acquire_owned().await
                && !self.cancelled()
            {
                self.execute_node(graph, node_ix).await;
            }
            node_ix
        }
        .boxed_local()
    }

    /// Execute a single node's items sequentially.
    ///
    /// A node may have multiple items (from `&&`-split commands). Items are executed
    /// in order; if any item fails, `execute_leaf` cancels the `fast_fail_token`
    /// and remaining items are skipped (preserving `&&` semantics).
    async fn execute_node(&self, graph: &ExecutionGraph, node_ix: ExecutionNodeIndex) {
        let task_execution = &graph.graph[node_ix];

        for item in &task_execution.items {
            if self.cancelled() {
                return;
            }
            match &item.kind {
                ExecutionItemKind::Leaf(leaf_kind) => {
                    self.execute_leaf(&item.execution_item_display, leaf_kind).boxed_local().await;
                }
                ExecutionItemKind::Expanded(nested_graph) => {
                    self.execute_expanded_graph(nested_graph).boxed_local().await;
                }
            }
        }
    }

    /// Execute a single leaf item (in-process command or spawned process).
    ///
    /// Creates a [`LeafExecutionReporter`] from the graph reporter and delegates
    /// to the appropriate execution method. On failure (non-zero exit or
    /// infrastructure error), cancels the `fast_fail_token`.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_leaf(&self, display: &ExecutionItemDisplay, leaf_kind: &LeafExecutionKind) {
        // Borrow the reporter briefly to create the leaf reporter, then drop
        // the RefCell guard before any `.await` point.
        let mut leaf_reporter = self.reporter.borrow_mut().new_leaf_execution(display, leaf_kind);

        let failed = match leaf_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                // In-process (built-in) commands: caching is disabled, execute synchronously
                let mut stdio_config = leaf_reporter
                    .start(CacheStatus::Disabled(CacheDisabledReason::InProcessExecution));

                let execution_output = in_process_execution.execute();
                // Write output to the stdout writer from StdioConfig
                let _ = stdio_config.writers.stdout_writer.write_all(&execution_output.stdout);
                let _ = stdio_config.writers.stdout_writer.flush();

                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    None,
                );
                false
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                let outcome = execute_spawn(
                    leaf_reporter,
                    spawn_execution,
                    self.cache,
                    self.cache_base_path,
                    self.cache_dir,
                    self.program_name,
                    self.fast_fail_token.clone(),
                    self.interrupt_token.clone(),
                )
                .await;
                match outcome {
                    SpawnOutcome::CacheHit => false,
                    SpawnOutcome::Spawned(status) => !status.success(),
                    SpawnOutcome::Failed => true,
                }
            }
        };
        if failed {
            self.fast_fail_token.cancel();
        }
    }
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
    /// `Some` iff auto-input or auto-output tracking is on (`includes_auto` +
    /// successful IPC bind). Bundles fspy's input negative globs with the
    /// per-task IPC server that runner-aware tools talk to. Parts are borrowed
    /// in place during the wait/join; the struct is never moved out.
    tracking: Option<Tracking>,
}

/// Per-task tracking: fspy input-negative globs + IPC server handle.
/// Lifetime-tied to a single `execute_spawn` call.
struct Tracking {
    input_negative_globs: Vec<wax::Glob<'static>>,
    ipc_envs: Vec<(&'static OsStr, OsString)>,
    ipc_server_fut: LocalBoxFuture<'static, Result<Recorder, vite_task_server::Error>>,
    stop_accepting: StopAccepting,
}

/// Post-execution summary of what fspy observed for a single task. Used in the
/// cache-update step. Fields are cfg-agnostic so the downstream match logic
/// doesn't need `cfg(fspy)` — the value is only ever `Some` when tracking
/// happened (see the `let fspy_outcome = ...` fork in `execute_spawn`).
struct TrackingOutcome {
    path_reads: HashMap<RelativePathBuf, PathRead>,
    /// All paths the task wrote to. Consumed by `collect_and_archive_outputs`
    /// when `output_config.includes_auto` is set.
    path_writes: FxHashSet<RelativePathBuf>,
    /// First path that was both read and written during execution, if any.
    /// A non-empty value means caching this task is unsound.
    read_write_overlap: Option<RelativePathBuf>,
}

/// Execute a spawned process with cache-aware lifecycle.
///
/// This is a free function (not tied to `ExecutionContext`) so it can be reused
/// from both graph-based execution and standalone synthetic execution.
///
/// The full lifecycle is:
/// 1. Cache lookup (determines cache status)
/// 2. `leaf_reporter.start(cache_status)` → `StdioConfig`
/// 3. If cache hit: replay cached outputs via `StdioConfig` writers → finish
/// 4. Otherwise: `spawn()` with the chosen stdio mode, optionally `pipe_stdio()`
///    to drain, then `child.wait` → cache update → finish
///
/// Errors (cache lookup failure, spawn failure, cache update failure) are reported
/// through `leaf_reporter.finish()` and do not abort the caller.
#[tracing::instrument(level = "debug", skip_all)]
#[expect(
    clippy::too_many_lines,
    reason = "sequential cache check, execute, and update steps are clearer in one function"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "these are the unavoidable inputs for a free-function cache-aware spawn"
)]
pub async fn execute_spawn(
    mut leaf_reporter: Box<dyn LeafExecutionReporter>,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    cache_base_path: &Arc<AbsolutePath>,
    cache_dir: &AbsolutePath,
    program_name: &str,
    fast_fail_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> SpawnOutcome {
    let cache_metadata = spawn_execution.cache_metadata.as_ref();

    // 1. Determine cache status FIRST by trying cache hit.
    //    We need to know the status before calling start() so the reporter
    //    can display cache status immediately when execution begins.
    let (cache_status, cached_value, globbed_inputs) = if let Some(cache_metadata) = cache_metadata
    {
        // Compute globbed inputs from positive globs at execution time
        // Globs are already workspace-root-relative (resolved at task graph stage)
        let globbed_inputs = match compute_globbed_inputs(
            cache_base_path,
            &cache_metadata.input_config.positive_globs,
            &cache_metadata.input_config.negative_globs,
        ) {
            Ok(inputs) => inputs,
            Err(err) => {
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                );
                return SpawnOutcome::Failed;
            }
        };

        match cache.try_hit(cache_metadata, &globbed_inputs, cache_base_path).await {
            Ok(Ok(cached)) => (
                // Cache hit — we can replay the cached outputs
                CacheStatus::Hit { replayed_duration: cached.duration },
                Some(cached),
                globbed_inputs,
            ),
            Ok(Err(cache_miss)) => (
                // Cache miss — includes detailed reason (NotFound or FingerprintMismatch)
                CacheStatus::Miss(cache_miss),
                None,
                globbed_inputs,
            ),
            Err(err) => {
                // Cache lookup error — report through finish.
                // Note: start() is NOT called because we don't have a valid cache status.
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                );
                return SpawnOutcome::Failed;
            }
        }
    } else {
        // No cache metadata provided — caching is disabled for this task
        (CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata), None, BTreeMap::new())
    };

    // 2. Report execution start with the determined cache status.
    //    Returns StdioConfig with the reporter's suggestion and writers.
    let mut stdio_config = leaf_reporter.start(cache_status);

    // 3. If cache hit, replay outputs via the StdioConfig writers and finish early.
    //    No need to actually execute the command — just replay what was cached.
    if let Some(cached) = cached_value {
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
            if let Err(err) = archive::extract_output_archive(cache_base_path, &archive_path) {
                let err = err.context(vite_str::format!(
                    "failed to restore cached outputs from {}; the archive may have been deleted \
                     or corrupted. Run `{program_name} cache clean` to clear the cache.",
                    archive_path.as_path().display()
                ));
                leaf_reporter.finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit),
                    Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                );
                return SpawnOutcome::Failed;
            }
        }
        leaf_reporter.finish(
            None,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit),
            None,
        );
        return SpawnOutcome::CacheHit;
    }

    // 4. Build the execution mode. This folds the cache/fspy/stdio decisions
    //    and their associated state into a single value whose shape encodes
    //    the valid combinations. The inherited arm drops `stdio_config` here so
    //    we don't hold `std::io::Stdout` while the child writes to the same FD.
    //
    // ─────────────────────────────────────────────────────────────────────
    //  Before adding a new local variable alongside `mode`: think twice.
    //  Does it make sense for every variant, or only for some?  If it's
    //  variant-specific (only for `Cached`, only when fspy is on, etc.) put
    //  it inside the variant (or `CacheState`) so the compiler enforces the
    //  invariant at construction. Sibling locals drift out of sync with the
    //  mode and force re-derivation (`if let Some(_) = _`,
    //  `cache_metadata.is_some_and(_)`) at every downstream use site.
    // ─────────────────────────────────────────────────────────────────────
    let mut mode: ExecutionMode<'_> = match cache_metadata {
        Some(metadata) => {
            let tracking =
                if metadata.input_config.includes_auto || metadata.output_config.includes_auto {
                    // Resolve input negative globs for fspy path filtering
                    // (already workspace-root-relative). Output negatives are
                    // applied later in `collect_and_archive_outputs`.
                    let negatives = match metadata
                        .input_config
                        .negative_globs
                        .iter()
                        .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
                        .collect::<anyhow::Result<Vec<_>>>()
                    {
                        Ok(negs) => negs,
                        Err(err) => {
                            leaf_reporter.finish(
                                None,
                                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                                Some(ExecutionError::PostRunFingerprint(err)),
                            );
                            return SpawnOutcome::Failed;
                        }
                    };
                    // PLACEHOLDER: the IPC server isn't wired up yet on this
                    // branch. We construct an empty `Recorder` whose `Reports`
                    // never get any IPC traffic, and a `StopAccepting::noop`
                    // since no real server was bound. A follow-up will swap
                    // these out for `vite_task_server::serve(...)`.
                    let env_map: FxHashMap<Arc<OsStr>, Arc<OsStr>> = std::env::vars_os()
                        .map(|(k, v)| {
                            (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str()))
                        })
                        .collect();
                    let recorder = Recorder::new(env_map);
                    let driver: LocalBoxFuture<'static, Result<Recorder, vite_task_server::Error>> =
                        Box::pin(std::future::ready(Ok(recorder)));
                    Some(Tracking {
                        input_negative_globs: negatives,
                        ipc_envs: Vec::new(),
                        ipc_server_fut: driver,
                        stop_accepting: StopAccepting::noop(),
                    })
                } else {
                    None
                };
            ExecutionMode::Cached {
                pipe_writers: stdio_config.writers,
                state: CacheState { metadata, globbed_inputs, std_outputs: Vec::new(), tracking },
            }
        }
        None => ExecutionMode::Uncached {
            pipe_writers: (stdio_config.suggestion == StdioSuggestion::Piped)
                .then_some(stdio_config.writers),
        },
    };

    // 5. Derive the arguments for `spawn()` from the mode without consuming it.
    let (spawn_stdio, fspy_enabled) = match &mode {
        ExecutionMode::Cached { state, .. } => (SpawnStdio::Piped, state.tracking.is_some()),
        ExecutionMode::Uncached { pipe_writers: Some(_) } => (SpawnStdio::Piped, false),
        ExecutionMode::Uncached { pipe_writers: None } => (SpawnStdio::Inherited, false),
    };

    // Build the extra envs to inject: IPC connection info + (eventually)
    // napi addon path. Empty when tracking is off. The napi path is added
    // in a follow-up that wires up the real server.
    let extra_envs: Vec<(&OsStr, &OsStr)> = match &mode {
        ExecutionMode::Cached { state: CacheState { tracking: Some(t), .. }, .. } => {
            t.ipc_envs.iter().map(|(k, v)| (*k as &OsStr, v.as_os_str())).collect()
        }
        _ => Vec::new(),
    };

    // Measure end-to-end duration here — spawn() no longer tracks time.
    let start = Instant::now();

    // 6. Spawn. Returns pipes (Piped) or `None` (Inherited) plus a
    //    cancellation-aware wait future.
    let mut child = match spawn(
        &spawn_execution.spawn_command,
        fspy_enabled,
        spawn_stdio,
        fast_fail_token.clone(),
        extra_envs,
    )
    .await
    {
        Ok(child) => child,
        Err(err) => {
            leaf_reporter.finish(
                None,
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Spawn(err)),
            );
            return SpawnOutcome::Failed;
        }
    };

    // 7. Extract all mode-scoped borrows in one pass:
    //    - `sinks`: pipe writers + stdout capture slot (writes in place).
    //    - `stop_accepting` / `driver`: the IPC server's handles, if tracking
    //      is on. Disjoint field borrows inside the same match arm.
    let (sinks, stop_accepting, ipc_server_fut) = match &mut mode {
        ExecutionMode::Cached { pipe_writers, state } => {
            let sinks = Some(PipeSinks {
                stdout_writer: &mut pipe_writers.stdout_writer,
                stderr_writer: &mut pipe_writers.stderr_writer,
                capture: Some(&mut state.std_outputs),
            });
            let (stop_accepting, ipc_server_fut) = match &mut state.tracking {
                Some(t) => (Some(&t.stop_accepting), Some(&mut t.ipc_server_fut)),
                None => (None, None),
            };
            (sinks, stop_accepting, ipc_server_fut)
        }
        ExecutionMode::Uncached { pipe_writers: Some(pipe_writers) } => (
            Some(PipeSinks {
                stdout_writer: &mut pipe_writers.stdout_writer,
                stderr_writer: &mut pipe_writers.stderr_writer,
                capture: None,
            }),
            None,
            None,
        ),
        ExecutionMode::Uncached { pipe_writers: None } => (None, None, None),
    };

    // 8. Drive pipe_stdio + child.wait concurrently with the IPC driver.
    //    The driver must be polled during pipe drain — otherwise a tool doing
    //    a blocking `getEnv` can deadlock: child stalls on IPC, stdout stays
    //    open, pipe_stdio waits for EOF, driver never runs. `stop_accepting`
    //    fires after child.wait so the driver drains any in-flight clients.
    let child_fast_fail = fast_fail_token.clone();
    let child_work = async move {
        let pipe_result: anyhow::Result<()> = if let Some(sinks) = sinks {
            let stdout = child.stdout.take().expect("SpawnStdio::Piped yields a stdout pipe");
            let stderr = child.stderr.take().expect("SpawnStdio::Piped yields a stderr pipe");
            #[expect(
                clippy::large_futures,
                reason = "pipe_stdio streams child I/O and creates a large future"
            )]
            let r = pipe_stdio(stdout, stderr, sinks, child_fast_fail.clone()).await;
            r.map_err(anyhow::Error::from)
        } else {
            Ok(())
        };

        let wait_result = match pipe_result {
            Ok(()) => child.wait.await.map_err(anyhow::Error::from),
            Err(err) => {
                // Pipe failed — cancel so `child.wait` kills the child
                // instead of orphaning it. Still signal the server so it
                // can drain.
                child_fast_fail.cancel();
                let _ = child.wait.await;
                Err(err)
            }
        };

        if let Some(s) = stop_accepting {
            s.signal();
        }
        wait_result
    };

    // Box::pin to keep the child-and-pipe stack off the enclosing future:
    // pipe_stdio alone makes the combined future large enough to trip
    // clippy::large_futures on the non-driver branch.
    let child_work = Box::pin(child_work);
    // `None` iff tracking was off. Otherwise carries either the collected
    // reports (Ok) or the IPC server's error (Err). Consumed below: Ok drops
    // unused for step 5 (step 6 will use the reports); Err short-circuits
    // cache update to `IpcServerError`.
    let mut ipc_server_result: Option<Result<Reports, vite_task_server::Error>> = None;
    let wait_result = match ipc_server_fut {
        Some(ipc_server_fut) => {
            let (wait_result, join_result) = tokio::join!(child_work, ipc_server_fut);
            if let Err(e) = &join_result {
                tracing::warn!(?e, "IPC server failed; cache will not be updated");
            }
            ipc_server_result = Some(join_result.map(Recorder::into_reports));
            wait_result
        }
        None => child_work.await,
    };

    let outcome = match wait_result {
        Ok(outcome) => outcome,
        Err(err) => {
            leaf_reporter.finish(
                None,
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Spawn(err)),
            );
            return SpawnOutcome::Failed;
        }
    };
    // Extract reports, or short-circuit when the IPC server failed. An Err
    // here means reports may be incomplete: caching this run would risk
    // stale inputs/outputs, so skip all cache-related computation entirely.
    let reports: Option<Reports> = match ipc_server_result {
        Some(Ok(r)) => {
            tracing::debug!(?r, "runner-aware tools reported");
            Some(r)
        }
        None => None,
        Some(Err(err)) => {
            leaf_reporter.finish(
                Some(outcome.exit_status),
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::IpcServerError(err)),
                None,
            );
            return SpawnOutcome::Spawned(outcome.exit_status);
        }
    };
    let duration = start.elapsed();

    // 9. Cache update (only when we were in `Cached` mode). Errors during cache
    //    update are reported but do not affect the exit status we return.
    let (cache_update_status, cache_error) = 'cache_update: {
        if let ExecutionMode::Cached { state, .. } = mode {
            let CacheState { metadata, globbed_inputs, std_outputs, tracking } = state;
            let input_negative_globs = tracking.as_ref().map(|t| t.input_negative_globs.as_slice());

            // Runner-aware tools can short-circuit caching entirely via
            // `disableCache()` (e.g. a dev server with no deterministic output).
            if let Some(r) = reports.as_ref()
                && r.cache_disabled
            {
                break 'cache_update (
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::ToolRequested),
                    None,
                );
            }

            // Tool-reported paths to exclude from auto-tracking. Absolute paths
            // are normalized to workspace-relative; anything outside is dropped.
            let ignored_input_rels: FxHashSet<RelativePathBuf> = reports
                .as_ref()
                .map(|r| normalize_ignored_paths(&r.ignored_inputs, cache_base_path))
                .unwrap_or_default();
            let ignored_output_rels: FxHashSet<RelativePathBuf> = reports
                .as_ref()
                .map(|r| normalize_ignored_paths(&r.ignored_outputs, cache_base_path))
                .unwrap_or_default();

            // Post-execution summary of what fspy observed. `Some` iff fspy was
            // both requested (`tracking.is_some()` => input or output auto) and
            // compiled in (`cfg(fspy)`). On a `cfg(not(fspy))` build this is
            // always `None`, and the match below short-circuits to
            // `FspyUnsupported` when tracking was needed.
            //
            // `path_reads` is gated on `input_config.includes_auto`, filtered
            // by user-configured input negatives, and by tool-reported
            // `ignoreInput` paths. When input auto is disabled (even if fspy is
            // enabled for output tracking) no reads contribute to the
            // fingerprint or the read-write overlap check. `path_writes` is NOT
            // filtered here — output negatives and `ignoreOutput` are applied
            // later inside `collect_and_archive_outputs`. Keeping the two sides
            // separate avoids `input: ["!dist/**"]` accidentally dropping
            // writes to `dist/**`, which would break archive restoration.
            let fspy_outcome: Option<TrackingOutcome> = {
                #[cfg(fspy)]
                {
                    outcome.path_accesses.as_ref().map(|raw| {
                        let tracked = TrackedPathAccesses::from_raw(raw, cache_base_path);
                        let path_reads: HashMap<RelativePathBuf, PathRead> =
                            if metadata.input_config.includes_auto
                                && let Some(negatives) = input_negative_globs
                            {
                                tracked
                                    .path_reads
                                    .iter()
                                    .filter(|(path, _)| {
                                        !negatives.iter().any(|neg| neg.is_match(path.as_str()))
                                            && !is_ignored(path, &ignored_input_rels)
                                    })
                                    .map(|(path, read)| (path.clone(), *read))
                                    .collect()
                            } else {
                                HashMap::default()
                            };
                        let read_write_overlap = path_reads
                            .keys()
                            .find(|p| {
                                tracked.path_writes.contains(*p)
                                    && !is_ignored(p, &ignored_output_rels)
                            })
                            .cloned();
                        TrackingOutcome {
                            path_reads,
                            path_writes: tracked.path_writes,
                            read_write_overlap,
                        }
                    })
                }
                #[cfg(not(fspy))]
                {
                    None
                }
            };

            let cancelled = fast_fail_token.is_cancelled() || interrupt_token.is_cancelled();
            if cancelled {
                // Cancelled (Ctrl-C or sibling failure) — result is untrustworthy
                (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::Cancelled), None)
            } else if outcome.exit_status.success() {
                // fspy-inferred read-write overlap: the task wrote to a file it
                // also read (as an inferred input), so the prerun input hashes
                // are stale and caching is unsound. Reads excluded by input
                // negatives or by `ignoreInput` don't count — `path_reads` is
                // already filtered. Writes excluded by `ignoreOutput` were
                // discounted from the overlap check too.
                if let Some(TrackingOutcome { read_write_overlap: Some(path), .. }) = &fspy_outcome
                {
                    (
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified {
                            path: path.clone(),
                        }),
                        None,
                    )
                } else if fspy_outcome.is_none() && tracking.is_some() {
                    // Task requested fspy auto-inference but this binary was built
                    // without `cfg(fspy)`. Task ran, but we can't compute a valid
                    // cache entry without tracked path accesses.
                    (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::FspyUnsupported), None)
                } else {
                    // Collect tool-reported tracked envs for the post-run
                    // fingerprint. User-declared `env` wins — skip names that
                    // are already in the spawn fingerprint.
                    let tracked_envs = reports
                        .as_ref()
                        .map(|r| collect_tracked_envs(r, metadata))
                        .unwrap_or_default();
                    let tracked_env_globs =
                        reports.as_ref().map(collect_tracked_env_globs).unwrap_or_default();

                    // Paths already in globbed_inputs are skipped: the overlap check
                    // above guarantees no input modification, so the prerun hash is
                    // the correct post-exec hash.
                    let empty_path_reads = HashMap::default();
                    let path_reads =
                        fspy_outcome.as_ref().map_or(&empty_path_reads, |o| &o.path_reads);
                    match PostRunFingerprint::create(
                        path_reads,
                        cache_base_path,
                        &globbed_inputs,
                        tracked_envs,
                        tracked_env_globs,
                    ) {
                        Ok(post_run_fingerprint) => {
                            // Collect output files and create archive. Tool-reported
                            // `ignoreOutput` paths are excluded from archiving too.
                            let output_archive = match collect_and_archive_outputs(
                                metadata,
                                fspy_outcome.as_ref(),
                                &ignored_output_rels,
                                cache_base_path,
                                cache_dir,
                            ) {
                                Ok(archive) => archive,
                                Err(err) => {
                                    break 'cache_update (
                                        CacheUpdateStatus::NotUpdated(
                                            CacheNotUpdatedReason::CacheDisabled,
                                        ),
                                        Some(ExecutionError::Cache {
                                            kind: CacheErrorKind::Update,
                                            source: err,
                                        }),
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
                                    CacheUpdateStatus::NotUpdated(
                                        CacheNotUpdatedReason::CacheDisabled,
                                    ),
                                    Some(ExecutionError::Cache {
                                        kind: CacheErrorKind::Update,
                                        source: err,
                                    }),
                                ),
                            }
                        }
                        Err(err) => (
                            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                            Some(ExecutionError::PostRunFingerprint(err)),
                        ),
                    }
                }
            } else {
                // Execution failed with non-zero exit status — don't update cache
                (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::NonZeroExitStatus), None)
            }
        } else {
            // Caching was disabled for this task
            (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled), None)
        }
    };

    // 7. Finish the leaf execution with the result and optional cache error.
    //    Cache update/fingerprint failures are reported but do not affect the outcome —
    //    the process ran, so we return its actual exit status.
    leaf_reporter.finish(Some(outcome.exit_status), cache_update_status, cache_error);

    SpawnOutcome::Spawned(outcome.exit_status)
}

/// Normalize tool-reported absolute paths to workspace-relative. Paths outside
/// the workspace are dropped — they can't contribute to inputs or outputs.
fn normalize_ignored_paths(
    paths: &FxHashSet<Arc<AbsolutePath>>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    // On Windows, `workspace_root` may carry a `\\?\` extended-path prefix
    // (it does when the runner derived it from `std::fs::canonicalize`)
    // while a tool's `current_dir()`-based ignoreInput/ignoreOutput path
    // doesn't. `Path::strip_prefix` is a byte-exact comparison so the
    // prefix mismatch silently drops every tool-reported path. Pre-build
    // an alternate workspace root with the `\\?\` / `\\.\` / `\??\`
    // prefix dropped and try it as a fallback. `fspy_shared::NativePath::
    // strip_path_prefix` does the inverse (strips `\\?\` from incoming
    // fspy paths) so each side stays agnostic to how the other side
    // canonicalised.
    #[cfg(windows)]
    let workspace_root_stripped: Option<vite_path::AbsolutePathBuf> =
        windows_strip_verbatim_prefix(workspace_root.as_path().as_os_str());

    paths
        .iter()
        .filter_map(|p| {
            if let Some(rel) = p.strip_prefix(workspace_root).ok().flatten() {
                return Some(rel);
            }
            #[cfg(windows)]
            if let Some(alt_root) = workspace_root_stripped.as_ref() {
                if let Some(rel) = p.strip_prefix(alt_root).ok().flatten() {
                    return Some(rel);
                }
            }
            None
        })
        .collect()
}

/// Build an alternate workspace-root path by dropping a `\\?\`, `\\.\`,
/// or `\??\` prefix if present. Returns `None` when the input is already
/// in plain `C:\...` form (no fallback needed). Mirrors
/// `fspy_shared::NativePath::strip_path_prefix`'s helper so the inputs of
/// `strip_prefix` can match across `current_dir`-derived and
/// `canonicalize`-derived paths.
#[cfg(windows)]
#[expect(
    clippy::disallowed_types,
    reason = "OsStr-level prefix matching for Windows extended-path normalization"
)]
fn windows_strip_verbatim_prefix(p: &std::ffi::OsStr) -> Option<vite_path::AbsolutePathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = p.encode_wide().collect();
    for prefix in [r"\\?\", r"\\.\", r"\??\"] {
        let prefix_wide: Vec<u16> = prefix.encode_utf16().collect();
        if wide.starts_with(prefix_wide.as_slice()) {
            let stripped = std::ffi::OsString::from_wide(&wide[prefix_wide.len()..]);
            return vite_path::AbsolutePathBuf::new(std::path::PathBuf::from(stripped));
        }
    }
    None
}

/// Whether `path` is covered by any `ignored` entry. An ignored entry matches
/// itself (exact file) and everything under it (directory subtree).
fn is_ignored(path: &RelativePathBuf, ignored: &FxHashSet<RelativePathBuf>) -> bool {
    if ignored.is_empty() {
        return false;
    }
    if ignored.contains(path) {
        return true;
    }
    ignored.iter().any(|ig| path.strip_prefix(ig).is_some())
}

/// Select tool-reported env records to embed in the post-run fingerprint.
/// Only `tracked: true` records are included, and names that the user already
/// declared as fingerprinted are skipped (their value is already in the cache
/// key via the spawn fingerprint).
fn collect_tracked_envs(reports: &Reports, metadata: &CacheMetadata) -> BTreeMap<Str, Option<Str>> {
    let fingerprinted = &metadata.spawn_fingerprint.env_fingerprints().fingerprinted_envs;
    reports
        .env_records
        .iter()
        .filter(|(_, record)| record.tracked)
        .filter_map(|(name, record)| {
            let name_str = name.to_str()?;
            if fingerprinted.contains_key(name_str) {
                return None;
            }
            let value = record.value.as_ref().and_then(|v| v.to_str().map(Str::from));
            Some((Str::from(name_str), value))
        })
        .collect()
}

/// Select tool-reported env-glob records to embed in the post-run
/// fingerprint. Only `tracked: true` records are included, and the full
/// match-set is stored as-is.
///
/// Unlike [`collect_tracked_envs`], names already covered by the user's
/// declared `env` are *not* filtered out: lookup-time validation re-expands
/// the glob over the whole parent env (see [`PostRunFingerprint::validate`]),
/// so a filtered match-set would always diff as having `added` entries and
/// miss the cache deterministically. Storing user-declared names here is
/// harmless — a change to their value already shifts the spawn fingerprint,
/// invalidating the cache key before the post-run fingerprint is consulted.
fn collect_tracked_env_globs(reports: &Reports) -> BTreeMap<Str, BTreeMap<Str, Str>> {
    reports
        .env_glob_records
        .iter()
        .filter(|(_, record)| record.tracked)
        .map(|(pattern, record)| {
            let matches: BTreeMap<Str, Str> = record
                .matches
                .iter()
                .filter_map(|(name, value)| {
                    let name_str = name.to_str()?;
                    let value_str = value.to_str()?;
                    Some((Str::from(name_str), Str::from(value_str)))
                })
                .collect();
            (Str::from(pattern.as_ref()), matches)
        })
        .collect()
}

/// Collect output files and create a tar.zst archive in the cache directory.
///
/// Output files are determined by:
/// - fspy-tracked writes (when `output_config.includes_auto` is true)
/// - Positive output globs (always, if configured)
/// - Filtered by negative output globs
/// - Filtered by tool-reported `ignoreOutput` paths (auto writes only)
///
/// Returns `Some(archive_filename)` if files were archived, `None` if no output files.
fn collect_and_archive_outputs(
    cache_metadata: &vite_task_plan::cache_metadata::CacheMetadata,
    tracking: Option<&TrackingOutcome>,
    ignored_output_rels: &FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
    cache_dir: &AbsolutePath,
) -> anyhow::Result<Option<Str>> {
    let output_config = &cache_metadata.output_config;

    // Collect output files from auto-detection (fspy writes), excluding
    // anything the tool reported via `ignoreOutput`.
    let mut output_files: FxHashSet<RelativePathBuf> = FxHashSet::default();

    if output_config.includes_auto
        && let Some(t) = tracking
    {
        output_files
            .extend(t.path_writes.iter().filter(|p| !is_ignored(p, ignored_output_rels)).cloned());
    }

    // Collect output files from positive globs
    if !output_config.positive_globs.is_empty() {
        let glob_paths = glob::collect_glob_paths(
            workspace_root,
            &output_config.positive_globs,
            &output_config.negative_globs,
        )?;
        output_files.extend(glob_paths);
    }

    // Apply negative globs to auto-detected files
    if output_config.includes_auto && !output_config.negative_globs.is_empty() {
        let negatives: Vec<wax::Glob<'static>> = output_config
            .negative_globs
            .iter()
            .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
            .collect::<anyhow::Result<_>>()?;
        output_files.retain(|path| !negatives.iter().any(|neg| neg.is_match(path.as_str())));
    }

    if output_files.is_empty() {
        return Ok(None);
    }

    // Sort for deterministic archive content
    let mut sorted_files: Vec<RelativePathBuf> = output_files.into_iter().collect();
    sorted_files.sort();

    // Create archive with UUID filename
    let archive_name: Str = vite_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
    let archive_path = cache_dir.join(archive_name.as_str());

    archive::create_output_archive(workspace_root, &sorted_files, &archive_path)?;

    Ok(Some(archive_name))
}

impl Session<'_> {
    /// Execute an execution graph, reporting events through the provided reporter builder.
    ///
    /// Cache is initialized only if any leaf execution needs it. The reporter is built
    /// after cache initialization, so cache errors are reported directly to stderr
    /// without involving the reporter at all.
    ///
    /// Returns `Err(ExitStatus)` to indicate the caller should exit with the given status code.
    /// Returns `Ok(())` when all tasks succeeded.
    #[tracing::instrument(level = "debug", skip_all)]
    pub(crate) async fn execute_graph(
        &self,
        execution_graph: ExecutionGraph,
        builder: Box<dyn GraphExecutionReporterBuilder>,
        interrupt_token: CancellationToken,
    ) -> Result<(), ExitStatus> {
        // Initialize cache before building the reporter. Cache errors are reported
        // directly to stderr and cause an early exit, keeping the reporter flow clean
        // (the reporter's `finish()` no longer accepts graph-level error messages).
        let cache = match self.cache() {
            Ok(cache) => cache,
            #[expect(clippy::print_stderr, reason = "cache init errors bypass the reporter")]
            Err(err) => {
                eprintln!("Failed to initialize cache: {err}");
                return Err(ExitStatus::FAILURE);
            }
        };

        let reporter = RefCell::new(builder.build());

        let execution_context = ExecutionContext {
            reporter: &reporter,
            cache,
            cache_base_path: &self.workspace_path,
            cache_dir: &self.cache_path,
            program_name: self.program_name.as_str(),
            fast_fail_token: CancellationToken::new(),
            interrupt_token,
        };

        // Execute the graph with fast-fail: if any task fails, remaining tasks
        // are skipped. Leaf-level errors are reported through the reporter.
        execution_context.execute_expanded_graph(&execution_graph).await;

        // Leaf-level errors and non-zero exit statuses are tracked internally
        // by the reporter.
        reporter.into_inner().finish()
    }
}

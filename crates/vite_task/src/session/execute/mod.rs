pub mod fingerprint;
pub mod glob_inputs;
mod hash;
pub mod pipe;
pub mod spawn;
pub mod tracked_accesses;
#[cfg(windows)]
mod win_job;

use std::{cell::RefCell, collections::BTreeMap, io::Write as _, sync::Arc, time::Instant};

use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::FxHashMap;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, SpawnExecution,
    cache_metadata::CacheMetadata, execution_graph::ExecutionNodeIndex,
};

use self::{
    fingerprint::PostRunFingerprint,
    glob_inputs::compute_globbed_inputs,
    pipe::{PipeSinks, StdOutput, pipe_stdio},
    spawn::{SpawnStdio, spawn},
    tracked_accesses::TrackedPathAccesses,
};
use super::{
    cache::{CacheEntryValue, ExecutionCache},
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
                #[expect(
                    clippy::large_futures,
                    reason = "spawn execution with cache management creates large futures"
                )]
                let outcome = execute_spawn(
                    leaf_reporter,
                    spawn_execution,
                    self.cache,
                    self.cache_base_path,
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
    /// `Some` iff fspy is enabled (`includes_auto`). Holds the resolved
    /// negative globs used by [`TrackedPathAccesses::from_raw`] to filter
    /// tracked accesses. `None` means fspy tracking is off for this task.
    fspy_negatives: Option<Vec<wax::Glob<'static>>>,
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
pub async fn execute_spawn(
    mut leaf_reporter: Box<dyn LeafExecutionReporter>,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    cache_base_path: &Arc<AbsolutePath>,
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
            let fspy = if metadata.input_config.includes_auto {
                // Resolve negative globs for fspy path filtering
                // (already workspace-root-relative).
                match metadata
                    .input_config
                    .negative_globs
                    .iter()
                    .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
                    .collect::<anyhow::Result<Vec<_>>>()
                {
                    Ok(negs) => Some(negs),
                    Err(err) => {
                        leaf_reporter.finish(
                            None,
                            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                            Some(ExecutionError::PostRunFingerprint(err)),
                        );
                        return SpawnOutcome::Failed;
                    }
                }
            } else {
                None
            };
            ExecutionMode::Cached {
                pipe_writers: stdio_config.writers,
                state: CacheState {
                    metadata,
                    globbed_inputs,
                    std_outputs: Vec::new(),
                    fspy_negatives: fspy,
                },
            }
        }
        None => ExecutionMode::Uncached {
            pipe_writers: (stdio_config.suggestion == StdioSuggestion::Piped)
                .then_some(stdio_config.writers),
        },
    };

    // 5. Derive the arguments for `spawn()` from the mode without consuming it.
    let (spawn_stdio, fspy_enabled) = match &mode {
        ExecutionMode::Cached { state, .. } => (SpawnStdio::Piped, state.fspy_negatives.is_some()),
        ExecutionMode::Uncached { pipe_writers: Some(_) } => (SpawnStdio::Piped, false),
        ExecutionMode::Uncached { pipe_writers: None } => (SpawnStdio::Inherited, false),
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

    // 7. Build `PipeSinks` by borrowing into `mode`. The drain fills
    //    `state.std_outputs` in place (via the borrow inside `capture`), so no
    //    post-drain transfer is needed. `sinks` is `None` only in the
    //    inherited-uncached case, where there are no pipes to drain.
    let sinks: Option<PipeSinks<'_>> = match &mut mode {
        ExecutionMode::Cached { pipe_writers, state } => Some(PipeSinks {
            stdout_writer: &mut pipe_writers.stdout_writer,
            stderr_writer: &mut pipe_writers.stderr_writer,
            capture: Some(&mut state.std_outputs),
        }),
        ExecutionMode::Uncached { pipe_writers: Some(pipe_writers) } => Some(PipeSinks {
            stdout_writer: &mut pipe_writers.stdout_writer,
            stderr_writer: &mut pipe_writers.stderr_writer,
            capture: None,
        }),
        ExecutionMode::Uncached { pipe_writers: None } => None,
    };

    if let Some(sinks) = sinks {
        let stdout = child.stdout.take().expect("SpawnStdio::Piped yields a stdout pipe");
        let stderr = child.stderr.take().expect("SpawnStdio::Piped yields a stderr pipe");
        #[expect(
            clippy::large_futures,
            reason = "pipe_stdio streams child I/O and creates a large future"
        )]
        let pipe_result = pipe_stdio(stdout, stderr, sinks, fast_fail_token.clone()).await;
        if let Err(err) = pipe_result {
            // Cancel so `child.wait` kills the child instead of orphaning it.
            fast_fail_token.cancel();
            let _ = child.wait.await;
            leaf_reporter.finish(
                None,
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Spawn(err.into())),
            );
            return SpawnOutcome::Failed;
        }
    }

    // 8. Wait for exit (handles cancellation internally).
    let outcome = match child.wait.await {
        Ok(outcome) => outcome,
        Err(err) => {
            leaf_reporter.finish(
                None,
                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                Some(ExecutionError::Spawn(err.into())),
            );
            return SpawnOutcome::Failed;
        }
    };
    let duration = start.elapsed();

    // 9. Cache update (only when we were in `Cached` mode). Errors during cache
    //    update are reported but do not affect the exit status we return.
    let (cache_update_status, cache_error) = if let ExecutionMode::Cached { state, .. } = mode {
        let CacheState { metadata, globbed_inputs, std_outputs, fspy_negatives } = state;

        // Normalize fspy accesses. `zip` gives `Some` iff fspy was enabled
        // (both outcome.path_accesses and fspy_negatives are Some together).
        let path_accesses = outcome
            .path_accesses
            .as_ref()
            .zip(fspy_negatives.as_deref())
            .map(|(raw, negs)| TrackedPathAccesses::from_raw(raw, cache_base_path, negs));

        let cancelled = fast_fail_token.is_cancelled() || interrupt_token.is_cancelled();
        if cancelled {
            // Cancelled (Ctrl-C or sibling failure) — result is untrustworthy
            (CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::Cancelled), None)
        } else if outcome.exit_status.success() {
            // Check for read-write overlap: if the task wrote to any file it also
            // read, the inputs were modified during execution — don't cache.
            // Note: this only checks fspy-inferred reads, not globbed_inputs keys.
            // A task that writes to a glob-matched file without reading it causes
            // perpetual cache misses (glob detects the hash change) but not a
            // correctness bug, so we don't handle that case here.
            if let Some(path) = path_accesses
                .as_ref()
                .and_then(|pa| pa.path_reads.keys().find(|p| pa.path_writes.contains(*p)))
            {
                (
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified {
                        path: path.clone(),
                    }),
                    None,
                )
            } else {
                // path_reads is empty when inference is disabled (path_accesses is None)
                let empty_path_reads = HashMap::default();
                let path_reads =
                    path_accesses.as_ref().map_or(&empty_path_reads, |pa| &pa.path_reads);

                // Execution succeeded — attempt to create fingerprint and update cache.
                // Paths already in globbed_inputs are skipped: Rule 1 (above) guarantees
                // no input modification, so the prerun hash is the correct post-exec hash.
                match PostRunFingerprint::create(path_reads, cache_base_path, &globbed_inputs) {
                    Ok(post_run_fingerprint) => {
                        let new_cache_value = CacheEntryValue {
                            post_run_fingerprint,
                            std_outputs: std_outputs.into(),
                            duration,
                            globbed_inputs,
                        };
                        match cache.update(metadata, new_cache_value).await {
                            Ok(()) => (CacheUpdateStatus::Updated, None),
                            Err(err) => (
                                CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
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
    };

    // 7. Finish the leaf execution with the result and optional cache error.
    //    Cache update/fingerprint failures are reported but do not affect the outcome —
    //    the process ran, so we return its actual exit status.
    leaf_reporter.finish(Some(outcome.exit_status), cache_update_status, cache_error);

    SpawnOutcome::Spawned(outcome.exit_status)
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

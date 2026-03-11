pub mod fingerprint;
pub mod glob_inputs;
pub mod spawn;

use std::{cell::RefCell, collections::BTreeMap, process::Stdio, rc::Rc, sync::Arc};

use futures_util::{FutureExt, StreamExt as _, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::FxHashMap;
use tokio::io::AsyncWriteExt as _;
use vite_path::AbsolutePath;
use vite_task_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, SpawnCommand,
    SpawnExecution, execution_graph::ExecutionNodeIndex,
};

use self::{
    fingerprint::PostRunFingerprint,
    glob_inputs::compute_globbed_inputs,
    spawn::{SpawnResult, TrackedPathAccesses, spawn_with_tracking},
};
use super::{
    cache::{CacheEntryValue, ExecutionCache},
    event::{
        CacheDisabledReason, CacheErrorKind, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus,
        ExecutionError,
    },
    reporter::{
        ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder, LeafExecutionReporter,
        StdioSuggestion,
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
/// The `reporter` field is wrapped in `Rc<RefCell<>>` to allow concurrent task
/// futures to create leaf reporters from the shared graph reporter.
/// Cache fields are passed through to [`execute_spawn`] for cache-aware execution.
struct ExecutionContext<'a> {
    /// The graph-level reporter, used to create leaf reporters via `new_leaf_execution()`.
    /// Wrapped in `Rc<RefCell<>>` so concurrent task futures can briefly borrow it
    /// to create leaf reporters without holding the borrow across await points.
    reporter: Rc<RefCell<&'a mut dyn GraphExecutionReporter>>,
    /// The execution cache for looking up and storing cached results.
    cache: &'a ExecutionCache,
    /// Base path for resolving relative paths in cache entries.
    /// Typically the workspace root.
    cache_base_path: &'a Arc<AbsolutePath>,
}

impl ExecutionContext<'_> {
    /// Execute all tasks in an execution graph, respecting dependency order
    /// and the graph's concurrency limit.
    ///
    /// **Single-node fast path:** When the graph has at most one node, tasks are
    /// executed sequentially to preserve `StdioSuggestion::Inherited` (allowing
    /// direct terminal I/O for a single task).
    ///
    /// **Multi-node concurrent path:** Independent tasks (no dependency relationship)
    /// run concurrently up to `graph.concurrency`. A task only starts after all its
    /// dependencies have completed. On any failure, all in-flight tasks are cancelled
    /// and no new tasks are started (fail-fast).
    ///
    /// Returns `true` if all tasks succeeded, `false` if any task failed.
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(clippy::future_not_send, reason = "uses !Send types internally")]
    async fn execute_expanded_graph(
        &self,
        graph: &ExecutionGraph,
        all_ancestors_single_node: bool,
    ) -> bool {
        // Single-node fast path: preserve Inherited stdio for the sole task.
        if graph.graph.node_count() <= 1 {
            if let Some(node_ix) = graph.graph.node_indices().next()
                && !self.execute_node(graph, node_ix, all_ancestors_single_node).boxed_local().await
            {
                return false;
            }
            return true;
        }

        // Multi-node concurrent execution.
        self.execute_concurrent(graph).await
    }

    /// Concurrent scheduler: runs independent tasks in parallel up to the concurrency limit.
    ///
    /// Uses a ready-queue + `FuturesUnordered` approach:
    /// 1. Compute initial dependency counts (outgoing neighbor count per node).
    /// 2. Seed the ready queue with nodes that have zero dependencies.
    /// 3. Launch ready tasks into `FuturesUnordered`, up to the concurrency limit.
    /// 4. When a task completes, decrement dependency counts for its dependents
    ///    (incoming neighbors) and enqueue newly-ready tasks.
    /// 5. On failure: drop `FuturesUnordered` to cancel all in-flight tasks (fail-fast).
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(clippy::future_not_send, reason = "uses !Send types internally")]
    async fn execute_concurrent(&self, graph: &ExecutionGraph) -> bool {
        let concurrency = graph.concurrency;

        // Compute dependency counts: for each node, count outgoing neighbors (dependencies).
        let mut remaining_deps = FxHashMap::<ExecutionNodeIndex, usize>::with_capacity_and_hasher(
            graph.graph.node_count(),
            rustc_hash::FxBuildHasher,
        );
        let mut ready: Vec<ExecutionNodeIndex> = Vec::new();

        for node_ix in graph.graph.node_indices() {
            let dep_count = graph.graph.neighbors_directed(node_ix, Direction::Outgoing).count();
            if dep_count == 0 {
                ready.push(node_ix);
            } else {
                remaining_deps.insert(node_ix, dep_count);
            }
        }

        let mut in_flight = FuturesUnordered::new();

        loop {
            // Fill up to concurrency limit from the ready queue.
            while in_flight.len() < concurrency {
                if let Some(node_ix) = ready.pop() {
                    // Multi-node graph: all_ancestors_single_node is always false.
                    in_flight.push(
                        async move {
                            let success = self.execute_node(graph, node_ix, false).await;
                            (node_ix, success)
                        }
                        .boxed_local(),
                    );
                } else {
                    break;
                }
            }

            if in_flight.is_empty() {
                break;
            }

            // Wait for any task to complete.
            let Some((completed_ix, success)) = in_flight.next().await else {
                break;
            };

            if !success {
                // Fail-fast: drop all in-flight futures (cancels running tasks)
                // and stop scheduling new ones.
                drop(in_flight);
                return false;
            }

            // Notify dependents: decrement their remaining dependency counts.
            // Incoming neighbors of `completed_ix` are nodes that depend on it.
            for dependent in graph.graph.neighbors_directed(completed_ix, Direction::Incoming) {
                if let Some(count) = remaining_deps.get_mut(&dependent) {
                    *count -= 1;
                    if *count == 0 {
                        remaining_deps.remove(&dependent);
                        ready.push(dependent);
                    }
                }
            }
        }

        true
    }

    /// Execute all items within a single task node sequentially.
    ///
    /// A task's command may be split by `&&` into multiple items. Each item is
    /// executed in order; if any item fails, the remaining items are skipped.
    ///
    /// Returns `true` if all items succeeded, `false` if any failed.
    #[expect(clippy::future_not_send, reason = "uses !Send types internally")]
    async fn execute_node(
        &self,
        graph: &ExecutionGraph,
        node_ix: ExecutionNodeIndex,
        all_ancestors_single_node: bool,
    ) -> bool {
        let task_execution = &graph.graph[node_ix];

        for item in &task_execution.items {
            let success = match &item.kind {
                ExecutionItemKind::Leaf(leaf_kind) => {
                    self.execute_leaf(
                        &item.execution_item_display,
                        leaf_kind,
                        all_ancestors_single_node,
                    )
                    .boxed_local()
                    .await
                }
                ExecutionItemKind::Expanded(nested_graph) => {
                    self.execute_expanded_graph(
                        nested_graph,
                        all_ancestors_single_node && nested_graph.graph.node_count() == 1,
                    )
                    .boxed_local()
                    .await
                }
            };
            if !success {
                return false;
            }
        }
        true
    }

    /// Execute a single leaf item (in-process command or spawned process).
    ///
    /// Creates a [`LeafExecutionReporter`] from the graph reporter (briefly borrowing
    /// the `RefCell`) and delegates to the appropriate execution method.
    ///
    /// Returns `true` on success, `false` on failure.
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(clippy::future_not_send, reason = "uses !Send types internally")]
    async fn execute_leaf(
        &self,
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
        all_ancestors_single_node: bool,
    ) -> bool {
        // Briefly borrow the reporter to create a leaf reporter, then drop the borrow.
        let mut leaf_reporter = self.reporter.borrow_mut().new_leaf_execution(
            display,
            leaf_kind,
            all_ancestors_single_node,
        );

        match leaf_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                // In-process (built-in) commands: caching is disabled, execute synchronously
                let mut stdio_config = leaf_reporter
                    .start(CacheStatus::Disabled(CacheDisabledReason::InProcessExecution))
                    .await;

                let execution_output = in_process_execution.execute();
                // Write output to the stdout writer from StdioConfig
                let _ = stdio_config.stdout_writer.write_all(&execution_output.stdout).await;
                let _ = stdio_config.stdout_writer.flush().await;

                leaf_reporter
                    .finish(
                        None,
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        None,
                    )
                    .await;
                true
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                #[expect(
                    clippy::large_futures,
                    reason = "spawn execution with cache management creates large futures"
                )]
                match execute_spawn(
                    leaf_reporter,
                    spawn_execution,
                    self.cache,
                    self.cache_base_path,
                )
                .await
                {
                    SpawnOutcome::CacheHit => true,
                    SpawnOutcome::Spawned(status) => status.success(),
                    SpawnOutcome::Failed => false,
                }
            }
        }
    }
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
/// 4. If `Inherited` suggestion AND caching disabled: `spawn_inherited()` → finish
/// 5. Else (piped): `spawn_with_tracking()` with writers → cache update → finish
///
/// Errors (cache lookup failure, spawn failure, cache update failure) are reported
/// through `leaf_reporter.finish()` and do not abort the caller.
#[tracing::instrument(level = "debug", skip_all)]
#[expect(clippy::future_not_send, reason = "uses !Send types internally")]
#[expect(
    clippy::too_many_lines,
    reason = "sequential cache check, execute, and update steps are clearer in one function"
)]
pub async fn execute_spawn(
    mut leaf_reporter: Box<dyn LeafExecutionReporter>,
    spawn_execution: &SpawnExecution,
    cache: &ExecutionCache,
    cache_base_path: &Arc<AbsolutePath>,
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
                leaf_reporter
                    .finish(
                        None,
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                    )
                    .await;
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
                leaf_reporter
                    .finish(
                        None,
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        Some(ExecutionError::Cache { kind: CacheErrorKind::Lookup, source: err }),
                    )
                    .await;
                return SpawnOutcome::Failed;
            }
        }
    } else {
        // No cache metadata provided — caching is disabled for this task
        (CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata), None, BTreeMap::new())
    };

    // 2. Report execution start with the determined cache status.
    //    Returns StdioConfig with the reporter's suggestion and async writers.
    let mut stdio_config = leaf_reporter.start(cache_status).await;

    // 3. If cache hit, replay outputs via the StdioConfig writers and finish early.
    //    No need to actually execute the command — just replay what was cached.
    if let Some(cached) = cached_value {
        for output in cached.std_outputs.iter() {
            let writer: &mut (dyn tokio::io::AsyncWrite + Unpin) = match output.kind {
                spawn::OutputKind::StdOut => &mut stdio_config.stdout_writer,
                spawn::OutputKind::StdErr => &mut stdio_config.stderr_writer,
            };
            let _ = writer.write_all(&output.content).await;
            let _ = writer.flush().await;
        }
        leaf_reporter
            .finish(None, CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheHit), None)
            .await;
        return SpawnOutcome::CacheHit;
    }

    // 4. Determine actual stdio mode based on the suggestion AND cache state.
    //    Inherited stdio is only used when the reporter suggests it AND caching is
    //    completely disabled (no cache_metadata). If caching is enabled but missed,
    //    we still need piped mode to capture output for the cache update.
    let use_inherited =
        stdio_config.suggestion == StdioSuggestion::Inherited && cache_metadata.is_none();

    if use_inherited {
        // Inherited mode: all three stdio FDs (stdin, stdout, stderr) are inherited
        // from the parent process. No fspy tracking, no output capture.
        // Drop the StdioConfig writers before spawning to avoid holding tokio::io::Stdout
        // while the child also writes to the same FD.
        drop(stdio_config);

        match spawn_inherited(&spawn_execution.spawn_command).await {
            Ok(result) => {
                leaf_reporter
                    .finish(
                        Some(result.exit_status),
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        None,
                    )
                    .await;
                return SpawnOutcome::Spawned(result.exit_status);
            }
            Err(err) => {
                leaf_reporter
                    .finish(
                        None,
                        CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                        Some(ExecutionError::Spawn(err)),
                    )
                    .await;
                return SpawnOutcome::Failed;
            }
        }
    }

    // 5. Piped mode: execute spawn with tracking, streaming output to writers.
    //    - std_outputs: always captured when caching is enabled (for cache replay)
    //    - path_accesses: only tracked when includes_auto is true (fspy inference)
    let (mut std_outputs, mut path_accesses, cache_metadata_and_inputs) =
        cache_metadata.map_or((None, None, None), |cache_metadata| {
            let path_accesses = if cache_metadata.input_config.includes_auto {
                Some(TrackedPathAccesses::default())
            } else {
                None // Skip fspy when inference is disabled
            };
            (Some(Vec::new()), path_accesses, Some((cache_metadata, globbed_inputs)))
        });

    // Build negative globs for fspy path filtering (already workspace-root-relative)
    let resolved_negatives: Vec<wax::Glob<'static>> =
        if let Some((cache_metadata, _)) = &cache_metadata_and_inputs {
            match cache_metadata
                .input_config
                .negative_globs
                .iter()
                .map(|p| Ok(wax::Glob::new(p.as_str())?.into_owned()))
                .collect::<anyhow::Result<Vec<_>>>()
            {
                Ok(negs) => negs,
                Err(err) => {
                    leaf_reporter
                        .finish(
                            None,
                            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                            Some(ExecutionError::PostRunFingerprint(err)),
                        )
                        .await;
                    return SpawnOutcome::Failed;
                }
            }
        } else {
            Vec::new()
        };

    #[expect(
        clippy::large_futures,
        reason = "spawn_with_tracking manages process I/O and creates a large future"
    )]
    let result = match spawn_with_tracking(
        &spawn_execution.spawn_command,
        cache_base_path,
        &mut stdio_config.stdout_writer,
        &mut stdio_config.stderr_writer,
        std_outputs.as_mut(),
        path_accesses.as_mut(),
        &resolved_negatives,
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            leaf_reporter
                .finish(
                    None,
                    CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::CacheDisabled),
                    Some(ExecutionError::Spawn(err)),
                )
                .await;
            return SpawnOutcome::Failed;
        }
    };

    // 6. Update cache if successful and determine cache update status.
    //    Errors during cache update are terminal (reported through finish).
    let (cache_update_status, cache_error) = if let Some((cache_metadata, globbed_inputs)) =
        cache_metadata_and_inputs
    {
        if result.exit_status.success() {
            // path_reads is empty when inference is disabled (path_accesses is None)
            let empty_path_reads = HashMap::default();
            let path_reads = path_accesses.as_ref().map_or(&empty_path_reads, |pa| &pa.path_reads);

            // Execution succeeded — attempt to create fingerprint and update cache
            match PostRunFingerprint::create(path_reads, cache_base_path) {
                Ok(post_run_fingerprint) => {
                    let new_cache_value = CacheEntryValue {
                        post_run_fingerprint,
                        std_outputs: std_outputs.unwrap_or_default().into(),
                        duration: result.duration,
                        globbed_inputs,
                    };
                    match cache.update(cache_metadata, new_cache_value).await {
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
    leaf_reporter.finish(Some(result.exit_status), cache_update_status, cache_error).await;

    SpawnOutcome::Spawned(result.exit_status)
}

/// Spawn a command with all three stdio file descriptors inherited from the parent.
///
/// Used when the reporter suggests inherited stdio AND caching is disabled.
/// All three FDs (stdin, stdout, stderr) are inherited, allowing interactive input
/// and direct terminal output. No fspy tracking is performed since there's no
/// cache to update.
///
/// The child process will see `is_terminal() == true` for stdout/stderr when the
/// parent is running in a terminal. This is expected behavior.
#[tracing::instrument(level = "debug", skip_all)]
async fn spawn_inherited(spawn_command: &SpawnCommand) -> anyhow::Result<SpawnResult> {
    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(vite_str::Str::as_str));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());

    let start = std::time::Instant::now();
    let mut tokio_cmd = cmd.into_tokio_command();

    // Clear FD_CLOEXEC on stdio fds before exec. libuv (used by Node.js) marks
    // stdin/stdout/stderr as close-on-exec, which causes them to be closed when
    // the child process calls exec(). Without this fix, the child's fds 0-2 are
    // closed after exec and Node.js reopens them as /dev/null, losing all output.
    // See: https://github.com/libuv/libuv/issues/2062
    // SAFETY: The pre_exec closure only performs fcntl operations to clear
    // FD_CLOEXEC flags on stdio fds, which is safe in a post-fork context.
    #[cfg(unix)]
    unsafe {
        tokio_cmd.pre_exec(|| {
            use std::os::fd::BorrowedFd;

            use nix::{
                fcntl::{FcntlArg, FdFlag, fcntl},
                libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO},
            };
            for fd in [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO] {
                // SAFETY: fds 0-2 are always valid in a post-fork context
                let borrowed = BorrowedFd::borrow_raw(fd);
                if let Ok(flags) = fcntl(borrowed, FcntlArg::F_GETFD) {
                    let mut fd_flags = FdFlag::from_bits_retain(flags);
                    if fd_flags.contains(FdFlag::FD_CLOEXEC) {
                        fd_flags.remove(FdFlag::FD_CLOEXEC);
                        let _ = fcntl(borrowed, FcntlArg::F_SETFD(fd_flags));
                    }
                }
            }
            Ok(())
        });
    }

    let mut child = tokio_cmd.spawn()?;
    let exit_status = child.wait().await?;

    Ok(SpawnResult { exit_status, duration: start.elapsed() })
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
    #[expect(clippy::future_not_send, reason = "uses !Send types internally")]
    pub(crate) async fn execute_graph(
        &self,
        execution_graph: ExecutionGraph,
        builder: Box<dyn GraphExecutionReporterBuilder>,
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

        let mut reporter = builder.build();

        let execution_context = ExecutionContext {
            reporter: Rc::new(RefCell::new(&mut *reporter)),
            cache,
            cache_base_path: &self.workspace_path,
        };

        // Execute the graph. On failure, remaining tasks are cancelled (fail-fast).
        // Cycle detection is handled at plan time.
        let all_single_node = execution_graph.graph.node_count() == 1;
        execution_context.execute_expanded_graph(&execution_graph, all_single_node).await;

        // Leaf-level errors and non-zero exit statuses are tracked internally
        // by the reporter.
        reporter.finish().await
    }
}

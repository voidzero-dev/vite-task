//! DAG scheduling for execution graphs: decides *which* leaf runs *when*
//! (dependency order, concurrency limits, fast-fail), and hands each leaf to
//! [`execute_spawn`] which owns *how* a single spawn runs.

use std::{cell::RefCell, ffi::OsStr, io::Write as _, num::NonZeroUsize, sync::Arc};

use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::FxHashMap;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use vt_casefold::EnvName;
use vt_path::AbsolutePath;
use vt_plan::{
    ExecutionGraph, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind,
    execution_graph::ExecutionNodeIndex,
};

use super::{SpawnOutcome, execute_spawn};
use crate::{
    Session,
    session::{
        cache::ExecutionCache,
        event::{CacheDisabledReason, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus},
        reporter::{ExitStatus, GraphExecutionReporter, GraphExecutionReporterBuilder},
    },
};

/// If set, the reporter isn't told about the uploads still running when the
/// graph is done, so there is no message about them, but the `uploads-pending`
/// milestone is still emitted. Whether an upload is still running then depends
/// on how fast the remote cache responds, so tests set this to keep their
/// output stable.
const HIDE_PENDING_UPLOADS_ENV: &str = "VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS";

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
    /// Workspace root that relative paths in cache entries (inputs, outputs,
    /// archives) are resolved against.
    workspace_root: &'a Arc<AbsolutePath>,
    /// Directory where cache files (db, archives) are stored.
    cache_dir: &'a AbsolutePath,
    /// Public-facing program name (e.g. `vp`), used in user-facing error
    /// messages that suggest a CLI command (e.g. `cache clean`).
    program_name: &'a str,
    /// Token cancelled when a task fails in a graph without `--continue`.
    /// Kills in-flight child processes (via `start_kill` in spawn.rs).
    ///
    /// One token serves every nested graph, and graphs with `--continue`
    /// obey it too. That matches cancelling each graph on its own because a
    /// nested `--continue` can't be turned off: a graph without it has no
    /// ancestor with it, so its failure would cancel every graph up to the
    /// root anyway.
    fast_fail_token: CancellationToken,
    /// Token cancelled by Ctrl-C, and by fast-fail as a child of
    /// `fast_fail_token`. Prevents scheduling new tasks and caching results,
    /// and stops remote cache lookups. On Ctrl-C, running processes are
    /// left to handle SIGINT naturally.
    cancel_token: CancellationToken,
}

impl ExecutionContext<'_> {
    /// Returns true if execution has been cancelled, either by a task
    /// failure (fast-fail) or by Ctrl-C (interrupt).
    fn cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Execute all tasks in an execution graph concurrently, respecting dependencies.
    ///
    /// Uses a DAG scheduler: tasks whose dependencies have all completed are scheduled
    /// onto a `FuturesUnordered`, bounded by a per-graph `Semaphore` with
    /// `concurrency_limit` permits. Each recursive `Expanded` graph creates its own
    /// semaphore, so nested graphs have independent concurrency limits.
    ///
    /// Fast-fail: if a node fails, this cancels the `fast_fail_token` (killing
    /// in-flight child processes). Ctrl-C cancels the `cancel_token`.
    /// Either cancellation causes this method to close the semaphore, drain
    /// remaining futures, and return.
    ///
    /// If the graph has `continue_on_failure` (`--continue`), a failure cancels
    /// nothing. A failed node doesn't release its dependents, so they never
    /// start, and every other node still runs.
    ///
    /// Returns whether any node in the graph failed.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_expanded_graph(&self, graph: &ExecutionGraph) -> bool {
        if graph.graph.node_count() == 0 {
            return false;
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
        // On failure without `--continue`, cancel the token here rather than in
        // `execute_leaf`, so a nested `--continue` graph that failed also stops
        // this one. Then close the semaphore (so pending acquires fail
        // immediately) and drain.
        let mut any_failed = false;
        while let Some((completed_ix, failed)) = futures.next().await {
            any_failed |= failed;
            if failed && !graph.continue_on_failure {
                self.fast_fail_token.cancel();
            }
            if self.cancelled() {
                semaphore.close();
                while let Some((_, failed)) = futures.next().await {
                    any_failed |= failed;
                }
                return any_failed;
            }

            // A failed node doesn't release its dependents, so their counts
            // never reach zero.
            if failed {
                continue;
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
        any_failed
    }

    /// Create a future that acquires a semaphore permit, then executes a graph node.
    ///
    /// The future resolves to the node and whether it failed; the caller then
    /// decides whether the failure cancels the run. On semaphore closure or
    /// prior cancellation, the node is skipped and doesn't count as failed.
    fn spawn_node<'a>(
        &'a self,
        graph: &'a ExecutionGraph,
        node_ix: ExecutionNodeIndex,
        semaphore: &Arc<Semaphore>,
    ) -> LocalBoxFuture<'a, (ExecutionNodeIndex, bool)> {
        let sem = semaphore.clone();
        async move {
            let failed = if let Ok(_permit) = sem.acquire_owned().await
                && !self.cancelled()
            {
                self.execute_node(graph, node_ix).await
            } else {
                false
            };
            (node_ix, failed)
        }
        .boxed_local()
    }

    /// Execute a single node's items sequentially, returning whether one failed.
    ///
    /// A node may have multiple items (from `&&`-split commands). Items are executed
    /// in order; if any item fails, remaining items are skipped (preserving `&&`
    /// semantics). An `Expanded` item fails if any node in its nested graph fails.
    async fn execute_node(&self, graph: &ExecutionGraph, node_ix: ExecutionNodeIndex) -> bool {
        let task_execution = &graph.graph[node_ix];

        for item in &task_execution.items {
            if self.cancelled() {
                return false;
            }
            let failed = match &item.kind {
                ExecutionItemKind::Leaf(leaf_kind) => {
                    self.execute_leaf(&item.execution_item_display, leaf_kind).boxed_local().await
                }
                ExecutionItemKind::Expanded(nested_graph) => {
                    self.execute_expanded_graph(nested_graph).boxed_local().await
                }
            };
            if failed {
                return true;
            }
        }
        false
    }

    /// Execute a single leaf item (in-process command or spawned process).
    ///
    /// Creates a [`LeafExecutionReporter`](crate::session::reporter::LeafExecutionReporter)
    /// from the graph reporter and delegates to the appropriate execution
    /// method. Returns whether the leaf failed (non-zero exit or infrastructure
    /// error); the graph decides whether that cancels the run.
    ///
    /// A spawn gets a child of the `fast_fail_token`: fast-fail still kills
    /// it, and when the spawn cancels the token to kill its own process (after
    /// failing to forward its output), other tasks keep running.
    #[tracing::instrument(level = "debug", skip_all)]
    async fn execute_leaf(
        &self,
        display: &ExecutionItemDisplay,
        leaf_kind: &LeafExecutionKind,
    ) -> bool {
        // Borrow the reporter briefly to create the leaf reporter, then drop
        // the RefCell guard before any `.await` point.
        let mut leaf_reporter = self.reporter.borrow_mut().new_leaf_execution(display, leaf_kind);

        match leaf_kind {
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
                    self.workspace_root,
                    self.cache_dir,
                    self.program_name,
                    self.fast_fail_token.child_token(),
                    self.cancel_token.clone(),
                )
                .await;
                match outcome {
                    SpawnOutcome::CacheHit | SpawnOutcome::Cancelled => false,
                    SpawnOutcome::Spawned(status) => !status.success(),
                    SpawnOutcome::Failed => true,
                }
            }
        }
    }
}

impl Session<'_> {
    /// Execute an execution graph, reporting events through the provided reporter builder.
    ///
    /// Cache is initialized only if any leaf execution needs it. The reporter is built
    /// after cache initialization, so cache errors are reported directly to stderr
    /// without involving the reporter at all.
    ///
    /// `fast_fail_token` and `cancel_token` are described on [`ExecutionContext`].
    ///
    /// Uploads to the remote cache run in the background and can outlive
    /// their tasks. Once the graph is done, this waits for them before the
    /// summary, telling the reporter how many are left. `interrupt_token`,
    /// which only Ctrl-C cancels, cancels them instead. Fast-fail doesn't, so
    /// tasks that succeeded before another one failed are still uploaded.
    ///
    /// Returns `Err(ExitStatus)` to indicate the caller should exit with the given status code.
    /// Returns `Ok(())` when all tasks succeeded.
    #[tracing::instrument(level = "debug", skip_all)]
    pub(crate) async fn execute_graph(
        &self,
        execution_graph: ExecutionGraph,
        builder: Box<dyn GraphExecutionReporterBuilder>,
        fast_fail_token: CancellationToken,
        cancel_token: CancellationToken,
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
            workspace_root: &self.workspace_path,
            cache_dir: &self.cache_path,
            program_name: self.program_name.as_str(),
            fast_fail_token,
            cancel_token,
        };

        // Execute the graph with fast-fail: if any task fails, remaining tasks
        // are skipped. In graphs with `--continue`, only the failed tasks'
        // dependents are. Leaf-level errors are reported through the reporter,
        // which also decides the exit status, so whether a task failed isn't
        // needed here.
        execution_context.execute_expanded_graph(&execution_graph).await;

        // Nested graphs share the cache, so this waits for their uploads too.
        // After Ctrl-C, the uploads are cancelled without a message.
        let mut reporter = reporter.into_inner();
        let hide_pending_uploads =
            self.envs.contains_key(EnvName::from_ref(OsStr::new(HIDE_PENDING_UPLOADS_ENV)));
        if !interrupt_token.is_cancelled()
            && let Some(pending) = NonZeroUsize::new(cache.pending_uploads())
        {
            if !hide_pending_uploads {
                reporter.uploads_pending(pending);
            }
            vt_pty_terminal_test_client::mark_milestone("uploads-pending");
        }
        cache.wait_for_uploads(&interrupt_token).await;

        // Leaf-level errors, non-zero exit statuses, and failed uploads are
        // tracked internally by the reporter.
        reporter.finish()
    }
}

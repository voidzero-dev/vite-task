//! DAG scheduling for execution graphs: decides *which* leaf runs *when*
//! (dependency order, concurrency limits, fast-fail), and hands each leaf to
//! [`execute_spawn`] which owns *how* a single spawn runs.

use std::{cell::RefCell, io::Write as _, sync::Arc};

use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use petgraph::Direction;
use rustc_hash::FxHashMap;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use vite_path::AbsolutePath;
use vite_task_plan::{
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
    /// Creates a [`LeafExecutionReporter`](crate::session::reporter::LeafExecutionReporter)
    /// from the graph reporter and delegates to the appropriate execution
    /// method. On failure (non-zero exit or infrastructure error), cancels the
    /// `fast_fail_token`.
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
                    self.workspace_root,
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
            workspace_root: &self.workspace_path,
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

//! Task query: map a `PackageQuery` to a `TaskExecutionGraph`.
//!
//! # Two-stage model
//!
//! Stage 1 — package selection — is handled by `IndexedPackageGraph::resolve_query`
//! and produces a `DiGraphMap<PackageNodeIndex, ()>` (the *package subgraph*).
//!
//! Stage 2 — task mapping — is handled by `map_subgraph_to_tasks`:
//! - Packages that **have** the requested task are mapped to that task.
//! - Packages that **lack** the task are *reconnected*: each predecessor is wired
//!   directly to each successor, then the task-lacking node is removed. This preserves
//!   transitive ordering even when intermediate packages miss the task.
//!
//! After all task-lacking nodes have been removed, the remaining package subgraph
//! contains only task-having packages; edges map directly to task dependency edges.
//!
//! Explicit `dependsOn` dependencies are then added on top by `add_dependencies`.

use std::collections::{BTreeMap, BTreeSet};

use petgraph::{Direction, prelude::DiGraphMap};
use rustc_hash::FxHashMap;
use vt_str::Str;
use vt_workspace::PackageNodeIndex;
pub use vt_workspace::package_graph::{PackageQuery, PackageQueryResolveError};

use crate::{IndexedTaskGraph, TaskGraphLoadError, TaskId, TaskNode};

/// A task execution graph queried from a `TaskQuery`.
///
/// `tasks` maps each selected task's ID to the task and the IDs of the tasks
/// it runs after.
///
/// A task is `requested` when the user typed it on the CLI — i.e. it was
/// added by `map_subgraph_to_tasks` (stage 2), not reached only via
/// `dependsOn` expansion in `IndexedTaskGraph::add_dependencies` (stage 3).
///
/// For example, given `test` with `dependsOn: ["build"]` and the command
/// `vp run test some-filter`:
///
/// - `tasks` contains both `test` and `build`, and `test` depends on `build`.
/// - Only `test` is `requested`.
///
/// The planner uses this distinction to forward `some-filter` to `test`
/// while running `build` with no extra args.
#[derive(Debug, Default, Clone)]
pub struct TaskExecutionGraph<'g> {
    pub tasks: BTreeMap<TaskId, QueriedTask<'g>>,
}

/// A task selected by a [`TaskQuery`].
#[derive(Debug, Clone)]
pub struct QueriedTask<'g> {
    pub node: &'g TaskNode,

    /// The tasks this one runs after, all of which are in the same [`TaskExecutionGraph`].
    pub dependencies: BTreeSet<TaskId>,

    /// Whether the query selected this task itself, rather than through `dependsOn`.
    pub requested: bool,
}

/// A query for which tasks to run.
///
/// A `TaskQuery` must be **self-contained**: it fully describes which tasks
/// will be selected, without relying on ambient state such as cwd or
/// environment variables. For example, the implicit cwd is captured as a
/// `ContainingPackage(path)` selector inside [`PackageQuery`], so two
/// queries from different directories compare as unequal even though the
/// user typed the same CLI arguments.
///
/// This property is essential for the **skip rule** in task planning, which
/// compares the nested query against the parent query with `==`. If any
/// external context leaked into the comparison (or was excluded from it),
/// the skip rule would either miss legitimate recursion or incorrectly
/// suppress distinct expansions.
#[derive(Debug, PartialEq)]
pub struct TaskQuery {
    /// Which packages to select.
    pub package_query: PackageQuery,

    /// The task name to run within each selected package.
    pub task_name: Str,

    /// Whether to include explicit `dependsOn` dependencies from the task config.
    pub include_explicit_deps: bool,
}

/// The result of [`IndexedTaskGraph::query_tasks`].
#[derive(Debug)]
pub struct TaskQueryResult<'g> {
    /// The final execution graph for the selected tasks.
    ///
    /// May be empty if no selected packages have the requested task, or if no
    /// packages matched the filters. The caller distinguishes the two cases
    /// with [`Self::selected_package_count`].
    pub execution_graph: TaskExecutionGraph<'g>,

    /// Original `--filter` strings for inclusion filters that contributed no
    /// packages to the final selected set — either the core selector matched
    /// nothing, or the traversal (e.g. `^...`) collapsed an otherwise-matching
    /// seed down to zero.
    ///
    /// Omits synthetic filters (implicit cwd, `-w`) since the user didn't type them.
    /// Always empty when `PackageQuery::All` was used.
    pub unmatched_selectors: Vec<Str>,

    /// Number of packages in the resolved package subgraph (Stage 1 result),
    /// before any task mapping.
    ///
    /// `0` means the filter expression(s) selected no packages at all — this
    /// is what tells the caller "no packages matched the filter" rather than
    /// "packages were selected but none have the requested task".
    pub selected_package_count: usize,
}

/// Errors that can occur when querying the task graph.
#[derive(Debug, thiserror::Error)]
pub enum TaskQueryError {
    #[error(transparent)]
    PackageQueryResolve(#[from] PackageQueryResolveError),

    #[error(transparent)]
    TaskGraphLoad(#[from] TaskGraphLoadError),
}

impl IndexedTaskGraph<'_> {
    /// Query the task graph based on the given [`TaskQuery`].
    ///
    /// Returns a [`TaskQueryResult`] containing the execution graph and any
    /// unmatched selectors. The execution graph may be empty — the caller decides
    /// what to do in that case (show task selector, emit warnings, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`PackageQueryResolveError`] when an exact package name (from a
    /// `pkg#task` specifier) matches no package or several, and
    /// [`TaskGraphLoadError`] when the tasks of a selected package fail to load.
    ///
    /// # Order of operations
    ///
    /// 1. Resolve `PackageQuery` → package subgraph (Stage 1).
    /// 2. Map package subgraph → task execution graph, reconnecting task-lacking
    ///    packages (Stage 2).
    /// 3. Expand explicit `dependsOn` edges (if `include_explicit_deps`).
    pub async fn query_tasks(
        &self,
        query: &TaskQuery,
    ) -> Result<TaskQueryResult<'_>, TaskQueryError> {
        let mut execution_graph = TaskExecutionGraph::default();

        // Stage 1: resolve package selection.
        let resolution = self.package_graph().resolve_query(&query.package_query)?;
        let selected_package_count = resolution.package_subgraph.node_count();

        // Stage 2: map each selected package to its task node (with reconnection).
        self.map_subgraph_to_tasks(
            &resolution.package_subgraph,
            &query.task_name,
            &mut execution_graph,
        )
        .await?;

        // Expand explicit dependsOn edges (may add new task nodes from outside the subgraph).
        if query.include_explicit_deps {
            self.add_dependencies(&mut execution_graph).await?;
        }

        Ok(TaskQueryResult {
            execution_graph,
            unmatched_selectors: resolution.unmatched_selectors,
            selected_package_count,
        })
    }

    /// Map a package subgraph to a task execution graph.
    ///
    /// For packages **with** the task: add the task.
    ///
    /// For packages **without** the task: wire each predecessor directly to each
    /// successor (skip-intermediate reconnection), then remove the node. Working on
    /// a *mutable clone* of the subgraph ensures that reconnected edges are visible
    /// when processing subsequent task-lacking nodes in the same pass — transitive
    /// task-lacking chains are resolved correctly regardless of iteration order.
    ///
    /// After all task-lacking nodes are removed, every remaining node in `subgraph`
    /// is guaranteed to be in `pkg_to_task`. The index operator panics on a missing
    /// key — a panic here indicates a bug in the reconnection loop above.
    async fn map_subgraph_to_tasks<'g>(
        &'g self,
        package_subgraph: &DiGraphMap<PackageNodeIndex, ()>,
        task_name: &Str,
        execution_graph: &mut TaskExecutionGraph<'g>,
    ) -> Result<(), TaskGraphLoadError> {
        // Build the task-lookup map for all packages that have the requested task.
        let mut pkg_to_task = FxHashMap::<PackageNodeIndex, &'g TaskNode>::default();
        for package_index in package_subgraph.nodes() {
            let id = TaskId { package_index, task_name: task_name.clone() };
            if let Some(task) = self.task(&id).await? {
                pkg_to_task.insert(package_index, task);
            }
        }

        // Clone the subgraph so that reconnection edits are visible in subsequent iterations.
        let mut subgraph = package_subgraph.clone();

        // Reconnect and remove each task-lacking node.
        for pkg in package_subgraph.nodes() {
            if pkg_to_task.contains_key(&pkg) {
                continue; // this package has the task — leave it in
            }
            // Read pred/succ from the live (possibly already-modified) subgraph.
            let preds: Vec<_> = subgraph.neighbors_directed(pkg, Direction::Incoming).collect();
            let succs: Vec<_> = subgraph.neighbors_directed(pkg, Direction::Outgoing).collect();
            // Bridge: every predecessor connects directly to every successor.
            for &pred in &preds {
                for &succ in &succs {
                    subgraph.add_edge(pred, succ, ());
                }
            }
            subgraph.remove_node(pkg);
        }

        // Map remaining nodes and their edges to tasks.
        // Every node still in `subgraph` is in `pkg_to_task`; the index operator
        // panics on a missing key — that would be a bug in the loop above.
        //
        // All tasks added here are explicitly requested.
        for &task in pkg_to_task.values() {
            execution_graph.tasks.insert(
                task.id.clone(),
                QueriedTask { node: task, dependencies: BTreeSet::new(), requested: true },
            );
        }
        for (src, dst, ()) in subgraph.all_edges() {
            let dependency = pkg_to_task[&dst].id.clone();
            execution_graph
                .tasks
                .get_mut(&pkg_to_task[&src].id)
                .expect("every task in pkg_to_task was added above")
                .dependencies
                .insert(dependency);
        }
        Ok(())
    }

    /// Recursively add `dependsOn` dependencies to the execution graph.
    ///
    /// Starts from the tasks in `execution_graph` and follows their `dependsOn`,
    /// adding new tasks to the frontier until no new tasks are discovered.
    async fn add_dependencies<'g>(
        &'g self,
        execution_graph: &mut TaskExecutionGraph<'g>,
    ) -> Result<(), TaskGraphLoadError> {
        let mut frontier: Vec<TaskId> = execution_graph.tasks.keys().cloned().collect();

        // Continue until no new tasks are added to the frontier.
        //
        // Tasks added here are dependency-only tasks and must NOT be marked as
        // `requested` — the planner uses that distinction to decide whether to
        // forward CLI extra args to a task.
        while !frontier.is_empty() {
            let mut next_frontier = Vec::new();

            for from in frontier {
                for dependency in self.depends_on(&from).await? {
                    if !execution_graph.tasks.contains_key(&dependency.id) {
                        execution_graph.tasks.insert(
                            dependency.id.clone(),
                            QueriedTask {
                                node: dependency,
                                dependencies: BTreeSet::new(),
                                requested: false,
                            },
                        );
                        next_frontier.push(dependency.id.clone());
                    }
                    execution_graph
                        .tasks
                        .get_mut(&from)
                        .expect("frontier tasks are in the execution graph")
                        .dependencies
                        .insert(dependency.id.clone());
                }
            }

            frontier = next_frontier;
        }
        Ok(())
    }
}

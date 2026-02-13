pub mod cli;

use std::sync::Arc;

use petgraph::{prelude::DiGraphMap, visit::EdgeRef};
use rustc_hash::FxHashSet;
use serde::Serialize;
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{
    IndexedTaskGraph, SpecifierLookupError, TaskDependencyType, TaskNodeIndex,
    specifier::TaskSpecifier,
};

/// Different kinds of task queries.
#[derive(Debug)]
pub enum TaskQueryKind {
    /// A normal task query specifying task specifiers and options.
    /// The tasks will be searched in packages in task specifiers, or located from cwd.
    Normal {
        task_specifiers: FxHashSet<TaskSpecifier>,
        /// Where the query is being run from.
        cwd: Arc<AbsolutePath>,
        /// Whether to include topological dependencies
        include_topological_deps: bool,
    },
    /// A recursive task query specifying one or multiple task names.
    /// It will match all tasks with the given names across all packages with topological ordering.
    /// The whole workspace will be searched, so cwd is not relevant.
    Recursive { task_names: FxHashSet<Str> },
}

/// Represents a valid query for a task and its dependencies, usually issued from a CLI command `vp run ...`.
/// A query represented by this struct is always valid, but still may result in no tasks found.
#[derive(Debug)]
pub struct TaskQuery {
    /// The kind of task query
    pub kind: TaskQueryKind,
    /// Whether to include explicit dependencies
    pub include_explicit_deps: bool,
}

/// A task execution graph queried from a `TaskQuery`.
///
/// The nodes are task indices for `TaskGraph`.
/// The edges represent the final dependency relationships between tasks. No edge weights.
pub type TaskExecutionGraph = DiGraphMap<TaskNodeIndex, ()>;

#[derive(Debug, thiserror::Error, Serialize)]
#[error("The current working directory {cwd:?} is in not any package")]
pub struct PackageUnknownError {
    pub cwd: Arc<AbsolutePath>,
}

#[derive(Debug, thiserror::Error, Serialize)]
pub enum TaskQueryError {
    #[error("Failed to look up task from specifier: {specifier}")]
    SpecifierLookupError {
        specifier: TaskSpecifier,
        #[source]
        lookup_error: SpecifierLookupError<PackageUnknownError>,
    },
}

impl IndexedTaskGraph {
    /// Queries the task graph based on the given [`TaskQuery`] and returns the execution graph.
    ///
    /// # Errors
    ///
    /// Returns [`TaskQueryError::SpecifierLookupError`] if a task specifier cannot be resolved
    /// to a task in the graph.
    pub fn query_tasks(&self, query: TaskQuery) -> Result<TaskExecutionGraph, TaskQueryError> {
        let mut execution_graph = TaskExecutionGraph::default();

        let include_topologicial_deps = match &query.kind {
            TaskQueryKind::Normal { include_topological_deps, .. } => *include_topological_deps,
            TaskQueryKind::Recursive { .. } => true, // recursive means topological across all packages
        };

        // Add starting tasks without dependencies
        match query.kind {
            TaskQueryKind::Normal { task_specifiers, cwd, include_topological_deps } => {
                let package_index_from_cwd =
                    self.indexed_package_graph.get_package_index_from_cwd(&cwd);

                // For every task specifier, add matching tasks
                for specifier in task_specifiers {
                    // Find the starting task
                    let starting_task_result =
                        self.get_task_index_by_specifier(specifier.clone(), || {
                            package_index_from_cwd
                                .ok_or_else(|| PackageUnknownError { cwd: Arc::clone(&cwd) })
                        });

                    match starting_task_result {
                        Ok(starting_task) => {
                            // Found it, add to execution graph
                            execution_graph.add_node(starting_task);
                        }
                        // Task not found, but package located, and the query requests topological deps
                        // This happens when running `vp run --transitive taskName` in a package without `taskName`, but its dependencies have it.
                        Err(err @ SpecifierLookupError::TaskNameNotFound { package_index, .. })
                            if include_topological_deps =>
                        {
                            // try to find nearest task
                            let mut nearest_topological_tasks = Vec::<TaskNodeIndex>::new();
                            self.find_nearest_topological_tasks(
                                &specifier.task_name,
                                package_index,
                                &mut nearest_topological_tasks,
                            );
                            if nearest_topological_tasks.is_empty() {
                                // No nearest task found, return original error
                                return Err(TaskQueryError::SpecifierLookupError {
                                    specifier,
                                    lookup_error: err,
                                });
                            }
                            // Add nearest tasks to execution graph
                            // Topological dependencies of nearest tasks will be added later
                            for nearest_task in nearest_topological_tasks {
                                execution_graph.add_node(nearest_task);
                            }
                        }
                        Err(err) => {
                            // Not recoverable by finding nearest package, return error
                            return Err(TaskQueryError::SpecifierLookupError {
                                specifier,
                                lookup_error: err,
                            });
                        }
                    }
                }
            }
            TaskQueryKind::Recursive { task_names } => {
                // Add all tasks matching the names across all packages
                for task_index in self.task_graph.node_indices() {
                    let current_task_name =
                        self.task_graph[task_index].task_display.task_name.as_str();
                    if task_names.contains(current_task_name) {
                        execution_graph.add_node(task_index);
                    }
                }
            }
        }

        // Add dependencies as requested
        // The order matters: add topological dependencies first, then explicit dependencies.
        // We don't want to include topological dependencies of explicit dependencies even both types are requested.
        if include_topologicial_deps {
            self.add_dependencies(&mut execution_graph, TaskDependencyType::is_topological);
        }
        if query.include_explicit_deps {
            self.add_dependencies(&mut execution_graph, TaskDependencyType::is_explicit);
        }

        Ok(execution_graph)
    }

    /// Recursively add dependencies to the execution graph based on filtered edges in the task graph
    fn add_dependencies(
        &self,
        execution_graph: &mut TaskExecutionGraph,
        mut filter_edge: impl FnMut(TaskDependencyType) -> bool,
    ) {
        let mut current_starting_node_indices: FxHashSet<TaskNodeIndex> =
            execution_graph.nodes().collect();

        // Continue until no new nodes are added
        while !current_starting_node_indices.is_empty() {
            // Record newly added nodes in this iteration as starting nodes for next iteration
            let mut next_starting_node_indices = FxHashSet::<TaskNodeIndex>::default();

            for from_node in current_starting_node_indices {
                // For each starting node, traverse its outgoing edges
                for edge_ref in self.task_graph.edges(from_node) {
                    let to_node = edge_ref.target();
                    let dependency_type = edge_ref.weight();
                    if filter_edge(*dependency_type) {
                        let is_to_node_new = !execution_graph.contains_node(to_node);
                        // Add the dependency edge
                        execution_graph.add_edge(from_node, to_node, ());

                        // Add to_node for next iteration if it's newly added.
                        if is_to_node_new {
                            next_starting_node_indices.insert(to_node);
                        }
                    }
                }
            }
            current_starting_node_indices = next_starting_node_indices;
        }
    }
}

use core::task;
use std::{collections::HashSet, sync::Arc};

use clap::{Parser, error};
use petgraph::{prelude::DiGraphMap, visit::EdgeRef};
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{SpecifierLookupError, TaskGraph, TaskNodeIndex, specifier::TaskSpecifier};

/// Different kinds of task queries.
pub enum TaskQueryKind {
    /// A normal task query specifying task specifiers and options.
    Normal {
        task_specifiers: HashSet<TaskSpecifier>,
        /// Where the query is being run from.
        cwd: Arc<AbsolutePath>,
        /// Whether to include topological dependencies
        include_topological_deps: bool,
    },
    /// A recursive task query specifying only a task name.
    /// It will match all tasks with the given names across all packages with topological ordering.
    Resursive { task_names: HashSet<Str> },
}

/// Represents a valid query for a task and its dependencies, usually issued from a CLI command `vite run ...`.
/// A query represented by this struct is always valid, but still may result in no tasks found.
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

#[derive(Debug, thiserror::Error)]
#[error("The current working directory {cwd:?} is in not any package")]
pub struct PackageUnknownError {
    cwd: Arc<AbsolutePath>,
}

impl TaskGraph {
    pub fn query_tasks(
        &self,
        query: TaskQuery,
    ) -> Result<TaskExecutionGraph, SpecifierLookupError<PackageUnknownError>> {
        let mut execution_graph = TaskExecutionGraph::default();
        match query.kind {
            TaskQueryKind::Normal { task_specifiers, cwd, include_topological_deps } => {
                let package_index_from_cwd =
                    self.indexed_package_graph.get_package_index_from_cwd(&cwd);

                let mut nearest_topological_tasks = Vec::<TaskNodeIndex>::new();

                // For every task specifier
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
                        // This happens when running `vite run --transitive taskName` in a package without `taskName`, but its dependencies have it.
                        Err(err @ SpecifierLookupError::TaskNameNotFound { package_index, .. })
                            if include_topological_deps =>
                        {
                            // try to find nearest task
                            self.find_nearest_topological_tasks(
                                &specifier.task_name,
                                package_index,
                                &mut nearest_topological_tasks,
                            );
                            if nearest_topological_tasks.is_empty() {
                                // No nearest task found, return original error
                                return Err(err);
                            }
                            // Add nearest tasks to execution graph
                            for nearest_task in nearest_topological_tasks.drain(..) {
                                execution_graph.add_node(nearest_task);
                            }
                        }
                        Err(err) => {
                            // Not recoverable by finding nearest package, return error
                            return Err(err);
                        }
                    }
                }
                todo!()
            }
            TaskQueryKind::Resursive { task_names } => {
                // Add all tasks matching the names
                for task_index in self.task_graph.node_indices() {
                    let current_task_name = self.task_graph[task_index].task_id.task_name.as_str();
                    if task_names.contains(current_task_name) {
                        execution_graph.add_node(task_index);
                    }
                }
                // Add topological edges
                let mut topo_edges = Vec::<(TaskNodeIndex, TaskNodeIndex)>::with_capacity(
                    execution_graph.node_count(),
                );
                for task_index in execution_graph.nodes() {
                    // for each added task
                    // Go through its dependencies
                    for edge_ref in self.task_graph.edges(task_index) {
                        let dep_index = edge_ref.target();
                        if edge_ref.weight().is_topological()
                            && execution_graph.contains_node(dep_index)
                        {
                            // only add edge if it's topological and the dependency is also in the execution graph
                            topo_edges.push((task_index, dep_index));
                        }
                    }
                }
                for (source, target) in topo_edges {
                    execution_graph.add_edge(source, target, ());
                }
            }
        }
        Ok(execution_graph)
    }
}

/// Represents task query args of `vite run`
/// It will be converted to `TaskQuery`, but may be invalid, if so the error is returned early before loading the task graph.
#[derive(Debug, clap::Parser)]
pub struct CLITaskQuery {
    /// Specifies one or multiple tasks to run, in form of `packageName#taskName` or `taskName`.
    tasks: Vec<TaskSpecifier>,

    /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    recursive: bool,

    /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    transitive: bool,

    /// Do not run dependencies specified in `dependsOn` fields.
    #[clap(default_value = "false", long)]
    ignore_depends_on: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --recursive")]
    PackageNameSpecifiedWithRecursive { package_name: Str, task_name: Str },
}

impl CLITaskQuery {
    /// Convert to `TaskQuery`, or return an error if invalid.
    pub fn into_task_query(self, cwd: &Arc<AbsolutePath>) -> Result<TaskQuery, CLITaskQueryError> {
        let include_explicit_deps = !self.ignore_depends_on;

        let kind = if self.recursive {
            if self.transitive {
                return Err(CLITaskQueryError::RecursiveTransitiveConflict);
            }
            let task_names: HashSet<Str> = self
                .tasks
                .into_iter()
                .map(|s| {
                    if let Some(package_name) = s.package_name {
                        return Err(CLITaskQueryError::PackageNameSpecifiedWithRecursive {
                            package_name,
                            task_name: s.task_name,
                        });
                    }
                    Ok(s.task_name)
                })
                .collect::<Result<_, _>>()?;
            TaskQueryKind::Resursive { task_names }
        } else {
            TaskQueryKind::Normal {
                task_specifiers: self.tasks.into_iter().collect(),
                cwd: Arc::clone(cwd),
                include_topological_deps: self.transitive,
            }
        };
        Ok(TaskQuery { kind, include_explicit_deps })
    }
}

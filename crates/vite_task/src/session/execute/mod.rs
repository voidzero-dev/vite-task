use core::task;
use std::{borrow::Cow, ops::Range, path::Path, sync::Arc};

use futures_util::FutureExt;
use petgraph::{
    algo::{Cycle, toposort},
    graph::DiGraph,
};
use sha2::digest::typenum::Abs;
use vite_path::{AbsolutePath, RelativePathBuf, relative::InvalidPathDataError};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNodeIndex};
use vite_task_plan::{
    ExecutionItem, ExecutionItemKind, ExecutionPlan, LeafExecutionKind, SpawnExecution,
    TaskExecution,
    execution_graph::{ExecutionGraph, ExecutionIx, ExecutionNodeIndex},
};

use super::{
    cache::{ExecutionCache, ExecutionCacheKey},
    event::{
        CacheDisabledReason, CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId,
        ExecutionStartInfo, ExecutionStartedEvent, OutputKind,
    },
};
use crate::{
    Session,
    session::{
        SessionExecutionPlan,
        cache::{DirectExecutionCacheKey, UserTaskExecutionCacheKey},
    },
};

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("Path {path:?} is outside of the workspace {workspace_path:?}")]
    PathOutsideWorkspace { path: Arc<AbsolutePath>, workspace_path: Arc<AbsolutePath> },
    #[error("Path {path:?} contains characters that make it non-portable")]
    NonPortableRelativePath {
        path: Arc<Path>,
        #[source]
        error: InvalidPathDataError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("Cycle dependencies detected: {0:?}")]
    CycleDependencies(Cycle<ExecutionNodeIndex>),

    #[error(transparent)]
    PathError(#[from] PathError),
}

struct ExecutionContext<'a> {
    indexed_task_graph: Option<&'a IndexedTaskGraph>,
    event_handler: &'a mut (dyn FnMut(ExecutionEvent) + 'a),
    current_execution_id: ExecutionId,
    cache: &'a ExecutionCache,
    /// All relative paths in cache are relative to this base path
    cache_base_path: &'a Arc<AbsolutePath>,
}

/// The origin of the current execution item, either directly from CLI args, or from a task in the task graph
enum ExecutionOrigin<'a> {
    CLIArgs {
        args_without_program: &'a Arc<[Str]>,
        cwd: &'a Arc<AbsolutePath>,
    },
    UserTask {
        task_node_index: TaskNodeIndex,
        item: &'a ExecutionItem,
        item_index: usize,
        item_count: usize,
    },
}

impl ExecutionContext<'_> {
    fn strip_prefix_for_cache(
        &self,
        path: &Arc<AbsolutePath>,
    ) -> Result<RelativePathBuf, PathError> {
        match path.strip_prefix(&*self.cache_base_path) {
            Ok(Some(rel_path)) => Ok(rel_path),
            Ok(None) => Err(PathError::PathOutsideWorkspace {
                path: Arc::clone(path),
                workspace_path: Arc::clone(self.cache_base_path),
            }),
            Err(err) => Err(PathError::NonPortableRelativePath {
                path: err.stripped_path.into(),
                error: err.invalid_path_data_error,
            }),
        }
    }

    async fn execute_item_kind(
        &mut self,
        item_kind: &ExecutionItemKind,
        origin: ExecutionOrigin<'_>,
    ) -> Result<(), ExecuteError> {
        match item_kind {
            ExecutionItemKind::Expanded(graph) => {
                // clone for reversing edges and removing nodes
                let mut graph: DiGraph<&TaskExecution, (), ExecutionIx> =
                    graph.map(|_, task_execution| task_execution, |_, ()| ());

                // To be consistent with the package graph in vite_package_manager and the dependency graph definition in Wikipedia
                // https://en.wikipedia.org/wiki/Dependency_graph, we construct the graph with edges from dependents to dependencies
                // e.g. A -> B means A depends on B
                //
                // For execution we need to reverse the edges first before topological sorting,
                // so that tasks without dependencies are executed first
                graph.reverse(); // Run tasks without dependencies first

                // Always use topological sort to ensure the correct order of execution
                // or the task dependencies declaration is meaningless
                let node_indices = match toposort(&graph, None) {
                    Ok(ok) => ok,
                    Err(err) => return Err(ExecuteError::CycleDependencies(err)),
                };

                let ordered_executions =
                    node_indices.into_iter().map(|id| graph.remove_node(id).unwrap());
                for task_execution in ordered_executions {
                    let indexed_task_graph = self.indexed_task_graph.unwrap();
                    let task_display =
                        indexed_task_graph.display_task(task_execution.task_node_index);
                    for (item_index, item) in task_execution.items.iter().enumerate() {
                        self.execute_item_kind(
                            &item.kind,
                            ExecutionOrigin::UserTask {
                                item,
                                item_index,
                                item_count: task_execution.items.len(),
                                task_node_index: task_execution.task_node_index,
                            },
                        )
                        .boxed_local()
                        .await?;
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                self.execute_leaf(leaf_execution_kind, origin).await?;
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        leaf_execution_kind: &LeafExecutionKind,
        origin: ExecutionOrigin<'_>,
    ) -> Result<(), ExecuteError> {
        let start_info = match origin {
            ExecutionOrigin::CLIArgs { args_without_program, cwd } => ExecutionStartInfo {
                task_display_name: None,
                // display command with `vite` followed by the user supplied cli args
                command: vite_str::format!(
                    "{}",
                    std::iter::once(Cow::Borrowed("vite"))
                        .chain(
                            args_without_program
                                .iter()
                                .map(|s| shell_escape::escape(s.as_str().into()))
                        )
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                cwd: Arc::clone(&cwd),
            },
            ExecutionOrigin::UserTask { task_node_index, item, item_index, item_count } => {
                let indexed_task_graph = self.indexed_task_graph.expect("Task graph must have been loaded if there exists an execution associated with a task");
                let task_node = &indexed_task_graph.task_graph()[task_node_index];
                let command = &task_node.resolved_config.command[item.command_span.clone()];
                let task_display = indexed_task_graph.display_task(task_node_index);
                ExecutionStartInfo {
                    task_display_name: Some(if item_count > 1 {
                        vite_str::format!("{} ({})", task_display, item_index)
                    } else {
                        vite_str::format!("{}", task_display)
                    }),
                    command: command.into(),
                    cwd: Arc::clone(&item.plan_cwd),
                }
            }
        };

        let execution_id = self.current_execution_id;
        self.current_execution_id = self.current_execution_id.next();
        (self.event_handler)(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Start(start_info),
        });

        match leaf_execution_kind {
            LeafExecutionKind::InProcess(in_process_execution) => {
                let execution_output = in_process_execution.execute().await;
                (self.event_handler)(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Output {
                        kind: OutputKind::Stdout,
                        content: execution_output.stdout.into(),
                    },
                });
                (self.event_handler)(ExecutionEvent {
                    execution_id,
                    kind: ExecutionEventKind::Finish {
                        status: Some(0),
                        cache_status: CacheStatus::Disabled(
                            CacheDisabledReason::InProcessExecution,
                        ),
                    },
                });
            }
            LeafExecutionKind::Spawn(spawn_execution) => {
                self.execute_spawn(execution_id, origin, spawn_execution).await?;
            }
        }
        Ok(())
    }

    async fn execute_spawn(
        &mut self,
        execution_id: ExecutionId,
        origin: ExecutionOrigin<'_>,
        spawn_execution: &SpawnExecution,
    ) -> Result<(), ExecuteError> {
        let execution_cache_key = match origin {
            ExecutionOrigin::CLIArgs { args_without_program, cwd } => {
                ExecutionCacheKey::Direct(DirectExecutionCacheKey {
                    args_without_program: Arc::clone(&args_without_program),
                    plan_cwd: self.strip_prefix_for_cache(cwd)?,
                })
            }
            ExecutionOrigin::UserTask { task_node_index, item_index, .. } => {
                let indexed_task_graph = self.indexed_task_graph.expect("Task graph must have been loaded if there exists an execution associated with a task");
                let task_node = &indexed_task_graph.task_graph()[task_node_index];
                let package_path = indexed_task_graph.get_package_path_for_task(task_node_index);
                ExecutionCacheKey::UserTask(UserTaskExecutionCacheKey {
                    task_name: task_node.task_id.task_name.clone(),
                    package_path: self.strip_prefix_for_cache(&package_path)?,
                    and_item_index: item_index,
                })
            }
        };

        // let mut cmd = match &spawn_execution.command_kind {
        //     SpawnCommandKind::Program { program_path, args } => {
        //         let mut cmd = fspy::Command::new(program_path.as_path());
        //         cmd.args(args.iter().map(|arg| arg.as_str()));
        //         cmd
        //     }
        //     SpawnCommandKind::ShellScript { script, args } => {
        //         let mut cmd = if cfg!(windows) {
        //             let mut cmd = fspy::Command::new("cmd.exe");
        //             // https://github.com/nodejs/node/blob/dbd24b165128affb7468ca42f69edaf7e0d85a9a/lib/child_process.js#L633
        //             cmd.args(["/d", "/s", "/c"]);
        //             cmd
        //         } else {
        //             let mut cmd = fspy::Command::new("sh");
        //             cmd.args(["-c"]);
        //             cmd
        //         };

        //         let mut script = script.clone();
        //         for arg in args.iter() {
        //             script.push(' ');
        //             script.push_str(shell_escape::escape(arg.as_str().into()).as_ref());
        //         }
        //         cmd.arg(script);
        //         cmd
        //     }
        // };
        // cmd.envs(spawn_execution.all_envs.iter()).current_dir(&*spawn_execution.cwd);
        todo!()
    }
}

impl<'a, CustomSubcommand> Session<'a, CustomSubcommand> {
    pub async fn execute(
        &self,
        plan: SessionExecutionPlan,
        event_handler: &mut (dyn FnMut(ExecutionEvent) + '_),
    ) -> Result<(), ExecuteError> {
        let mut execution_context = ExecutionContext {
            indexed_task_graph: self.lazy_task_graph.try_get(),
            event_handler,
            current_execution_id: ExecutionId::zero(),
            cache: &self.cache,
            cache_base_path: &self.workspace_path,
        };
        execution_context
            .execute_item_kind(
                plan.plan.root_node(),
                ExecutionOrigin::CLIArgs {
                    args_without_program: &plan.cli_args_without_program,
                    cwd: &plan.cwd,
                },
            )
            .await
    }
}

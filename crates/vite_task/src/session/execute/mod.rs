use std::{borrow::Cow, path::Path, sync::Arc};

use futures_util::FutureExt;
use petgraph::{
    algo::{Cycle, toposort},
    graph::DiGraph,
};
use vite_path::{AbsolutePath, RelativePathBuf, relative::InvalidPathDataError};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNodeIndex, display::TaskDisplay};
use vite_task_plan::{
    ExecutionItem, ExecutionItemKind, ExecutionPlan, LeafExecutionKind, SpawnExecution,
    TaskExecution,
    execution_graph::{ExecutionIx, ExecutionNodeIndex},
};

use super::{
    cache::{ExecutionCache, ExecutionCacheKey},
    event::{
        CacheDisabledReason, CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId,
        ExecutionItemDisplay, OutputKind,
    },
};
use crate::{
    Session,
    session::cache::{DirectExecutionCacheKey, UserTaskExecutionCacheKey},
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
        task_display: TaskDisplay,
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
        task_display: Option<&TaskDisplay>,
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
                    let task_display = task_execution.task_display.clone();
                    for (item_index, item) in task_execution.items.iter().enumerate() {
                        self.execute_item_kind(&item.kind, Some(&task_execution.task_display))
                            .boxed_local()
                            .await?;
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                self.execute_leaf(leaf_execution_kind, task_display).await?;
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        leaf_execution_kind: &LeafExecutionKind,
        task_display: Option<&TaskDisplay>,
    ) -> Result<(), ExecuteError> {
        let start_info: ExecutionItemDisplay = todo!();

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
                self.execute_spawn(execution_id, todo!(), spawn_execution).await?;
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
        let execution_cache_key: ExecutionCacheKey = todo!();

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
        plan: ExecutionPlan,
        event_handler: &mut (dyn FnMut(ExecutionEvent) + '_),
    ) -> Result<(), ExecuteError> {
        let mut execution_context = ExecutionContext {
            indexed_task_graph: self.lazy_task_graph.try_get(),
            event_handler,
            current_execution_id: ExecutionId::zero(),
            cache: &self.cache,
            cache_base_path: &self.workspace_path,
        };
        Ok(())
        // execution_context
        //     .execute_item_kind(
        //         plan.plan.root_node(),
        //         ExecutionOrigin::CLIArgs {
        //             args_without_program: &plan.cli_args_without_program,
        //             cwd: &plan.cwd,
        //         },
        //     )
        //     .await
    }
}

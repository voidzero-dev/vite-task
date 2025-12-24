use core::task;
use std::sync::Arc;

use futures_util::FutureExt;
use petgraph::{
    algo::{Cycle, toposort},
    graph::DiGraph,
};
use vite_str::Str;
use vite_task_graph::IndexedTaskGraph;
use vite_task_plan::{
    ExecutionItemKind, ExecutionPlan, LeafExecutionKind, SpawnCommandKind, SpawnExecution,
    TaskExecution,
    execution_graph::{ExecutionGraph, ExecutionIx, ExecutionNodeIndex},
};

use super::{
    cache::{ExecutionCacheKey, TaskCache},
    event::{
        CacheDisabledReason, CacheStatus, ExecutionEvent, ExecutionEventKind, ExecutionId,
        ExecutionStartedEvent, OutputKind, TaskInfo,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("Cycle dependencies detected: {0:?}")]
    CycleDependencies(Cycle<ExecutionNodeIndex>),
}

struct ExecutionContext<'a> {
    indexed_task_graph: Option<&'a IndexedTaskGraph>,
    event_handler: &'a mut (dyn FnMut(ExecutionEvent) + 'a),
    current_execution_id: ExecutionId,
    cache: &'a TaskCache,
}

impl ExecutionContext<'_> {
    async fn execute_item_kind(
        &mut self,
        item_kind: &ExecutionItemKind,
        task_info: Option<TaskInfo>,
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
                    let task_command = indexed_task_graph.task_graph()
                        [task_execution.task_node_index]
                        .resolved_config
                        .command
                        .as_str();
                    let task_display =
                        indexed_task_graph.display_task(task_execution.task_node_index);
                    for (index, item) in task_execution.items.iter().enumerate() {
                        let item_command = &task_command[item.command_span.clone()];
                        let task_info = TaskInfo {
                            task_display_name: if task_execution.items.len() > 1 {
                                vite_str::format!("{} ({})", task_display, index)
                            } else {
                                vite_str::format!("{}", task_display)
                            },
                            command: item_command.into(),
                            plan_cwd: Arc::clone(&item.plan_cwd),
                        };
                        self.execute_item_kind(&item.kind, Some(task_info)).boxed_local().await?;
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                let execution_id = self.current_execution_id;
                self.current_execution_id = self.current_execution_id.next();

                self.execute_leaf(leaf_execution_kind, task_info, todo!()).await?;
            }
        }
        Ok(())
    }

    async fn execute_leaf(
        &mut self,
        leaf_execution_kind: &LeafExecutionKind,
        task_info: Option<TaskInfo>,
        task_run_cache_key: Option<ExecutionCacheKey>,
    ) -> Result<(), ExecuteError> {
        let execution_id = self.current_execution_id;
        self.current_execution_id = self.current_execution_id.next();

        (self.event_handler)(ExecutionEvent {
            execution_id,
            kind: ExecutionEventKind::Start { task_info },
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
                self.execute_spawn(execution_id, spawn_execution).await?;
            }
        }
        Ok(())
    }

    async fn execute_spawn(
        &mut self,
        execution_id: ExecutionId,
        spawn_execution: &SpawnExecution,
    ) -> Result<(), ExecuteError> {
        let mut cmd = match &spawn_execution.command_kind {
            SpawnCommandKind::Program { program, args } => {
                let mut cmd = fspy::Command::new(&*program);
                cmd.args(args.iter().map(|arg| arg.as_str()));
                cmd
            }
            SpawnCommandKind::ShellScript { script, args } => {
                let mut cmd = if cfg!(windows) {
                    let mut cmd = fspy::Command::new("cmd.exe");
                    // https://github.com/nodejs/node/blob/dbd24b165128affb7468ca42f69edaf7e0d85a9a/lib/child_process.js#L633
                    cmd.args(["/d", "/s", "/c"]);
                    cmd
                } else {
                    let mut cmd = fspy::Command::new("sh");
                    cmd.args(["-c"]);
                    cmd
                };
                cmd.arg(script);
                cmd
            }
        };
        cmd.envs(spawn_execution.all_envs.iter()).current_dir(&*spawn_execution.cwd);
        todo!()
    }
}

pub async fn execute_plan(
    plan: &ExecutionPlan,
    args: &Arc<[Str]>,
    indexed_task_graph: Option<&IndexedTaskGraph>,
    event_handler: &mut (dyn FnMut(ExecutionEvent) + '_),
    cache: &TaskCache,
) -> Result<(), ExecuteError> {
    let mut execution_context = ExecutionContext {
        indexed_task_graph,
        event_handler,
        current_execution_id: ExecutionId::zero(),
        cache,
    };
    execution_context.execute_item_kind(plan.root_node(), None).await
}

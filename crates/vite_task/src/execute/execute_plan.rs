use core::task;

use petgraph::{
    algo::{Cycle, toposort},
    graph::DiGraph,
};
use vite_task_graph::IndexedTaskGraph;
use vite_task_plan::{
    ExecutionItemKind, ExecutionPlan, LeafExecutionKind, TaskExecution,
    execution_graph::{ExecutionGraph, ExecutionIx, ExecutionNodeIndex},
};

use super::reporter::Reporter;
use crate::execute::reporter;

#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    #[error("Cycle dependencies detected: {0:?}")]
    CycleDependencies(Cycle<ExecutionNodeIndex>),
}

struct ExecutionContext<'a> {
    indexed_task_graph: Option<&'a IndexedTaskGraph>,
    reporter: &'a mut (dyn Reporter + 'a),
    last_execution_id: u32,
}

impl ExecutionContext<'_> {
    fn execute_item_kind(
        &mut self,
        item_kind: &ExecutionItemKind,
        task_display: &str,
        command: &str,
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

                        let task_display_str = if task_execution.items.len() > 1 {
                            vite_str::format!("{} ({})", task_display, index)
                        } else {
                            vite_str::format!("{}", task_display)
                        };

                        for item in &task_execution.items {
                            self.execute_item_kind(&item.kind, command, &task_display_str)?;
                        }
                    }
                }
            }
            ExecutionItemKind::Leaf(leaf_execution_kind) => {
                self.execute_leaf(leaf_execution_kind, command)?;
            }
        }
        Ok(())
    }

    fn execute_leaf(
        &mut self,
        leaf_execution_kind: &LeafExecutionKind,
        command: &str,
    ) -> Result<(), ExecuteError> {
        Ok(())
    }
}

pub fn execute_plan(
    plan: &ExecutionPlan,
    indexed_task_graph: Option<&IndexedTaskGraph>,
    reporter: &mut (dyn Reporter + '_),
    command: &str,
) -> Result<(), ExecuteError> {
    let mut execution_context =
        ExecutionContext { indexed_task_graph, reporter, last_execution_id: 0 };
    execution_context.execute_item_kind(plan.root_node(), command, command)
}

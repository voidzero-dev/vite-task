use petgraph::graph::DiGraph;

use crate::{ExecutionGraphNode, ExpansionArgs, PlanContext};

/*


#[derive(Debug, thiserror::Error)]
pub enum ExecutionExpansionError {
    #[error("Failed to load task graph")]
    TaskGraphLoadError(
        #[source]
        #[from]
        vite_task_graph::TaskGraphLoadError,
    ),
    #[error("Failed to query tasks from task graph")]
    TaskQueryError(
        #[source]
        #[from]
        vite_task_graph::query::TaskQueryError,
    ),
}

impl ExpandedExecutionItem {
    pub async fn expand_from(
        parsed_args: ExpansionArgs,
        context: PlanContext<'_>,
    ) -> Result<Self, ExecutionExpansionError> {
        match parsed_args {
            ExpansionArgs::QueryTaskGraph { query, plan_options: _ } => {
                // Load the task graph
                let indexed_task_graph = context.callbacks.load_task_graph().await?;

                // Expand the task query into execution graph
                let task_execution_graph = indexed_task_graph.query_tasks(query)?;

                // Resolve each task node into execution nodes
                let task_graph = indexed_task_graph.task_graph();
                for (from_task_index, to_task_index, ()) in task_execution_graph.all_edges() {
                    let from_task = &task_graph[from_task_index];
                    let to_task = &task_graph[to_task_index];
                }
            }
            ExpansionArgs::Synthetic { name, extra_args } => {
                todo!()
            }
        }
        todo!()
    }
}

*/

pub async fn expand_into_execution_graph(
    expansion_args: ExpansionArgs,
    context: PlanContext<'_>,
) -> DiGraph<ExecutionGraphNode, ()> {
    match expansion_args {
        ExpansionArgs::QueryTaskGraph { query, plan_options } => {
            let indexed_task_graph =
                context.callbacks.load_task_graph().await.expect("Failed to load task graph");
        }
        ExpansionArgs::Synthetic { name, extra_args } => {}
    }
    todo!()
}

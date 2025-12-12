use core::task;
use std::{ffi::OsStr, sync::Arc};

use petgraph::graph::DiGraph;
use vite_shell::try_parse_as_and_list;
use vite_task_graph::{IndexedTaskGraph, TaskGraph, TaskNodeIndex};

use crate::{
    ExecutionGraphNode, ExecutionItem, ExecutionItemKind, LeafExecutionItem, PlanContext,
    QueryTasksSubcommand, Subcommand, error::Error,
};

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

pub async fn resolve_task_to_execution_node(
    indexed_task_graph: &IndexedTaskGraph,
    task_node_index: TaskNodeIndex,
    context: PlanContext,
) -> Result<ExecutionGraphNode, Error> {
    let task_node = &indexed_task_graph.task_graph()[task_node_index];

    // TODO: variable expansion (https://crates.io/crates/shellexpand) BEFORE parsing
    let command_str = task_node.resolved_config.command.as_str();
    if let Some(parsed_subcommands) = try_parse_as_and_list(command_str) {
        let mut items = Vec::<ExecutionItem>::with_capacity(parsed_subcommands.len());
        for (and_item, add_item_span) in parsed_subcommands {
            // Try to parse the args of an and_item to known vite subcommands like `run -r build`
            let parsed_subcommand = context
                .callbacks
                .parse_args(&and_item.program, &and_item.args)
                .map_err(|error| Error::CallbackParseArgsError {
                    package_path: Arc::clone(
                        indexed_task_graph.get_package_path(task_node.task_id.package_index),
                    ),
                    subcommand: (&command_str[add_item_span.clone()]).into(),
                    error,
                })?;

            // Create a new context with additional envs from `ENV_VAR=value` items
            let new_context = context
                .with_envs(and_item.envs.iter().map(|(name, value)| (name.clone(), value.clone())));

            let execution_item_kind: ExecutionItemKind = match parsed_subcommand {
                Some(Subcommand::QueryTasks(query_tasks_subcommand)) => {
                    // Expand task query like `vite run -r build`
                    let execution_graph =
                        expand_into_execution_graph(query_tasks_subcommand, new_context).await?;
                    ExecutionItemKind::Expanded(execution_graph)
                }
                Some(Subcommand::Synthetic { name, extra_args }) => {
                    // Synthetic task, like `vite lint`
                    todo!()
                }
                None => {
                    todo!()
                    // Normal 3rd party tool command (like `tsc --noEmit`)
                    // ExecutionItemKind::Leaf(LeafExecutionItem {
                    //     resolved_envs: todo!(),
                    //     cwd: Arc::clone(&new_context.cwd),
                    //     command_kind: todo!(),
                    // })
                }
            };
            items.push(ExecutionItem { command_span: add_item_span, kind: execution_item_kind });
        }
    } else {
    }

    todo!()
}

/// Expand the parsed command arguments (like `-r build`) into an execution graph.
pub async fn expand_into_execution_graph(
    query_tasks_subcommand: QueryTasksSubcommand,
    context: PlanContext,
) -> Result<DiGraph<ExecutionGraphNode, ()>, Error> {
    let indexed_task_graph = context.callbacks.load_task_graph().await?;

    // Query matching tasks from the task graph
    let task_node_index_graph = indexed_task_graph.query_tasks(query_tasks_subcommand.query)?;

    let task_graph = indexed_task_graph.task_graph();
    for (from_task_index, to_task_index, ()) in task_node_index_graph.all_edges() {
        let from_task = &task_graph[from_task_index];
        let to_task = &task_graph[to_task_index];
    }

    // Subcommand::Synthetic { name, extra_args } => {}
    todo!()
}

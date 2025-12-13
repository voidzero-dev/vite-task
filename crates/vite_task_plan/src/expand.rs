use core::task;
use std::{ffi::OsStr, sync::Arc, task::Context};

use petgraph::graph::DiGraph;
use vite_shell::try_parse_as_and_list;
use vite_task_graph::{IndexedTaskGraph, TaskGraph, TaskNodeIndex};

use crate::{
    ExecutionGraphNode, ExecutionItem, ExecutionItemKind, LeafExecutionItem, PlanContext,
    QueryTasksSubcommand, ResolvedCacheConfig, Subcommand,
    envs::ResolvedEnvs,
    error::{Error, TaskPlanError},
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

pub async fn resolve_task_to_execution_node(
    indexed_task_graph: &IndexedTaskGraph,
    task_node_index: TaskNodeIndex,
    context: PlanContext<'_>,
) -> Result<ExecutionGraphNode, TaskPlanError> {
    let task_node = &indexed_task_graph.task_graph()[task_node_index];

    if !context.task_call_stack.insert(task_node_index) {
        todo!("report cycle error");
    }

    // TODO: variable expansion (https://crates.io/crates/shellexpand) BEFORE parsing
    let command_str = task_node.resolved_config.command.as_str();
    if let Some(parsed_subcommands) = try_parse_as_and_list(command_str) {
        let mut items = Vec::<ExecutionItem>::with_capacity(parsed_subcommands.len());
        for (and_item, add_item_span) in parsed_subcommands {
            // Try to parse the args of an and_item to known vite subcommands like `run -r build`
            let parsed_subcommand = context
                .callbacks
                .parse_args(&and_item.program, &and_item.args)
                .map_err(|error| TaskPlanError::CallbackParseArgsError {
                    subcommand: (&command_str[add_item_span.clone()]).into(),
                    error,
                })?;

            let execution_item_kind: ExecutionItemKind = match parsed_subcommand {
                // Expand task query like `vite run -r build`
                Some(Subcommand::QueryTasks(query_tasks_subcommand)) => {
                    // Add envs from the prefix to the new context

                    // let execution_graph =
                    //     expand_into_execution_graph(query_tasks_subcommand, new_context).await?;
                    // ExecutionItemKind::Expanded(execution_graph)
                    todo!()
                }
                Some(Subcommand::Synthetic { name, extra_args }) => {
                    // Synthetic task, like `vite lint`
                    todo!()
                }
                None => {
                    // Normal 3rd party tool command (like `tsc --noEmit`)
                    // ExecutionItemKind::Leaf(LeafExecutionItem {
                    //     resolved_cache_config: task_node.resolved_config.cache_config.map(
                    //         |cache_config| ResolvedCacheConfig {
                    //             resolved_envs: ResolvedEnvs::resolve(
                    //                 todo!(),
                    //                 todo!(),
                    //                 todo!(),
                    //                 todo!(),
                    //             )?,
                    //         },
                    //     ),
                    //     cwd: Arc::clone(&new_context.cwd),
                    //     command_kind: todo!(),
                    // })
                    todo!()
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
    context: PlanContext<'_>,
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

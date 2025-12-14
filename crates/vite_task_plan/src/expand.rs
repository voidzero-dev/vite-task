use std::{borrow::Cow, collections::HashMap, sync::Arc};

use vite_shell::try_parse_as_and_list;
use vite_task_graph::TaskNodeIndex;

use crate::{
    ExecutionItem, ExecutionItemKind, PlanContext, ResolvedCacheConfig, SpawnCommandKind,
    SpawnExecutionItem, TaskExecution,
    envs::ResolvedEnvs,
    error::{Error, TaskPlanErrorKind, TaskPlanErrorKindResultExt},
    execution_graph::{ExecutionGraph, ExecutionNodeIndex},
    task_request::{QueryTaskRequest, TaskRequest},
};

pub fn plan_task_as_execution_node(
    task_node_index: TaskNodeIndex,
    mut context: PlanContext<'_>,
) -> Result<TaskExecution, Error> {
    // Check for cycles in the task call stack.
    context
        .check_cycle(task_node_index)
        .map_err(TaskPlanErrorKind::TaskCycleDetected)
        .with_task_call_stack(&context)?;

    // Prepend {package_path}/node_modules/.bin to PATH
    let package_node_modules_bin_path = context
        .indexed_task_graph()
        .get_package_path_for_task(task_node_index)
        .join("node_modules")
        .join(".bin");
    context
        .prepend_path(&package_node_modules_bin_path)
        .map_err(|join_paths_error| TaskPlanErrorKind::AddNodeModulesBinPathError {
            task_display: context.indexed_task_graph().display_task(task_node_index),
            join_paths_error,
        })
        .with_task_call_stack(&context)?;

    let task_node = &context.indexed_task_graph().task_graph()[task_node_index];

    // TODO: variable expansion (https://crates.io/crates/shellexpand) BEFORE parsing
    let command_str = task_node.resolved_config.command.as_str();

    // Try to parse the command string as a list of subcommands separated by `&&`
    if let Some(parsed_subcommands) = try_parse_as_and_list(command_str) {
        let mut items = Vec::<ExecutionItem>::with_capacity(parsed_subcommands.len());
        for (and_item, add_item_span) in parsed_subcommands {
            // Duplicate the context before modifying it for each and_item
            let mut context = context.duplicate();
            context.push_stack_frame(task_node_index, add_item_span.clone());

            // Add prefix envs to the context
            context.add_envs(and_item.envs.iter());

            // Try to parse the args of an and_item to a task request like `run -r build`
            let task_request = context
                .callbacks()
                .parse_as_task_request(&and_item.program, &and_item.args)
                .map_err(|error| TaskPlanErrorKind::ParseAsTaskRequestError { error })
                .with_task_call_stack(&context)?;

            let execution_item_kind: ExecutionItemKind = match task_request {
                // Expand task query like `vite run -r build`
                Some(TaskRequest::Query(query_task_request)) => {
                    let execution_graph =
                        plan_task_request_as_execution_graph(query_task_request, context)?;
                    ExecutionItemKind::Expanded(execution_graph)
                }
                // Synthetic task, like `vite lint`
                Some(TaskRequest::Synthetic(synthetic_task_request)) => {
                    todo!()
                }
                // Normal 3rd party tool command (like `tsc --noEmit`)
                None => {
                    // all envs available in the current context, wrapped in Cow to allow mutation by cache configs.
                    let mut all_envs = Cow::Borrowed(context.envs());

                    let mut resolved_cache_config = None;
                    if let Some(cache_config) = &task_node.resolved_config.cache_config {
                        // Resolve envs according cache configs
                        let resolved_envs =
                            ResolvedEnvs::resolve(all_envs.to_mut(), &cache_config.env_config)
                                .map_err(TaskPlanErrorKind::ResolveEnvError)
                                .with_task_call_stack(&context)?;
                        resolved_cache_config = Some(ResolvedCacheConfig { resolved_envs });
                    }
                    ExecutionItemKind::Spawn(SpawnExecutionItem {
                        all_envs: Arc::new(all_envs.into_owned()),
                        resolved_cache_config,
                        cwd: Arc::clone(&task_node.resolved_config.cwd),
                        command_kind: SpawnCommandKind::Program {
                            program: and_item.program,
                            args: and_item.args.into(),
                        },
                    })
                }
            };
            items.push(ExecutionItem { command_span: add_item_span, kind: execution_item_kind });
        }
    } else {
    }

    todo!()
}

/// Expand the parsed task request (like `run -r build`/`exec tsc`/`lint`) into an execution graph.
pub fn plan_task_request_as_execution_graph(
    query_task_request: QueryTaskRequest,
    mut context: PlanContext<'_>,
) -> Result<ExecutionGraph, Error> {
    // Query matching tasks from the task graph
    let task_node_index_graph = context
        .indexed_task_graph()
        .query_tasks(query_task_request.query)
        .map_err(TaskPlanErrorKind::TaskQueryError)
        .with_task_call_stack(&context)?;

    let mut execution_node_indices_by_task_index =
        HashMap::<TaskNodeIndex, ExecutionNodeIndex>::with_capacity(
            task_node_index_graph.node_count(),
        );

    let mut execution_graph = ExecutionGraph::with_capacity(
        task_node_index_graph.node_count(),
        task_node_index_graph.edge_count(),
    );

    // Plan each task node as execution nodes
    for task_index in task_node_index_graph.nodes() {
        let task_execution = plan_task_as_execution_node(task_index, context.duplicate())?;
        execution_node_indices_by_task_index
            .insert(task_index, execution_graph.add_node(task_execution));
    }

    // Add edges between execution nodes according to task dependencies
    for (from_task_index, to_task_index, ()) in task_node_index_graph.all_edges() {
        execution_graph.add_edge(
            execution_node_indices_by_task_index[&from_task_index],
            execution_node_indices_by_task_index[&to_task_index],
            (),
        );
    }

    Ok(execution_graph)
}

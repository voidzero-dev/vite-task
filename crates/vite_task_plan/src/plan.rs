use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    sync::Arc,
};

use vite_path::AbsolutePath;
use vite_shell::try_parse_as_and_list;
use vite_str::Str;
use vite_task_graph::{TaskNodeIndex, config::ResolvedTaskConfig};

use crate::{
    ExecutionItem, ExecutionItemKind, LeafExecutionKind, PlanContext, ResolvedCacheConfig,
    SpawnCommandKind, SpawnExecution, TaskExecution,
    envs::{self, ResolvedEnvs},
    error::{Error, TaskPlanErrorKind, TaskPlanErrorKindResultExt},
    execution_graph::{ExecutionGraph, ExecutionNodeIndex},
    in_process::InProcessExecution,
    plan_request::{PlanRequest, QueryPlanRequest, SyntheticPlanRequest},
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

    let mut items = Vec::<ExecutionItem>::new();

    // Try to parse the command string as a list of subcommands separated by `&&`
    if let Some(parsed_subcommands) = try_parse_as_and_list(command_str) {
        for (and_item, add_item_span) in parsed_subcommands {
            // Duplicate the context before modifying it for each and_item
            let mut context = context.duplicate();
            context.push_stack_frame(task_node_index, add_item_span.clone());

            // Check for builtin commands like `echo ...`
            if let Some(builtin_execution) =
                InProcessExecution::get_builtin_execution(&and_item.program, and_item.args.iter())
            {
                items.push(ExecutionItem {
                    command_span: add_item_span,
                    kind: ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(builtin_execution)),
                });
                continue;
            }

            // Try to parse the args of an and_item to a task request like `run -r build`
            let task_request = context
                .callbacks()
                .parse_as_task_request(&and_item.program, &and_item.args)
                .map_err(|error| TaskPlanErrorKind::ParsePlanRequestError { error })
                .with_task_call_stack(&context)?;

            let execution_item_kind: ExecutionItemKind = match task_request {
                // Expand task query like `vite run -r build`
                Some(PlanRequest::Query(query_task_request)) => {
                    // Add prefix envs to the context
                    context.add_envs(and_item.envs.iter());
                    let execution_graph =
                        plan_query_request_as_execution_graph(query_task_request, context)?;
                    ExecutionItemKind::Expanded(execution_graph)
                }
                // Synthetic task, like `vite lint`
                Some(PlanRequest::Synthetic(synthetic_task_request)) => {
                    todo!()
                }
                // Normal 3rd party tool command (like `tsc --noEmit`)
                None => {
                    let spawn_execution = plan_spawn_execution(
                        &and_item.envs,
                        SpawnCommandKind::Program {
                            program: and_item.program,
                            args: and_item.args.into(),
                        },
                        &task_node.resolved_config,
                        context,
                    )?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
            };
            items.push(ExecutionItem { command_span: add_item_span, kind: execution_item_kind });
        }
    } else {
        let spawn_execution = plan_spawn_execution(
            &BTreeMap::new(),
            SpawnCommandKind::ShellScript(command_str.into()),
            &task_node.resolved_config,
            context,
        )?;
        items.push(ExecutionItem {
            command_span: 0..command_str.len(),
            kind: ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution)),
        });
    }

    Ok(TaskExecution { task_node_index, items })
}

pub fn plan_synthetic_request_as_spawn_execution(
    synthetic_task_request: SyntheticPlanRequest,
    cwd: &Arc<AbsolutePath>,
    envs: &BTreeMap<Arc<OsStr>, Arc<OsStr>>,
) -> Result<SpawnExecution, Error> {
    let resolved_config =
        ResolvedTaskConfig::resolve(synthetic_task_request.user_config, &cwd, None)
            .expect("Command conflict/missing for synthetic task should never happen");

    // SpawnExecution {

    // }
    todo!()
}

fn plan_spawn_execution(
    prefix_envs: &BTreeMap<Str, Str>,
    command_kind: SpawnCommandKind,
    resolved_config: &ResolvedTaskConfig,
    context: PlanContext<'_>,
) -> Result<SpawnExecution, Error> {
    // all envs available in the current context
    let mut all_envs = context.envs().clone();

    let mut resolved_cache_config = None;
    if let Some(cache_config) = &resolved_config.resolved_options.cache_config {
        // Resolve envs according cache configs
        let mut resolved_envs = ResolvedEnvs::resolve(&mut all_envs, &cache_config.env_config)
            .map_err(TaskPlanErrorKind::ResolveEnvError)
            .with_task_call_stack(&context)?;

        // Add prefix envs to fingerprinted envs
        resolved_envs
            .fingerprinted_envs
            .extend(prefix_envs.iter().map(|(name, value)| (name.clone(), value.as_str().into())));
        resolved_cache_config = Some(ResolvedCacheConfig { resolved_envs });
    }

    // Add prefix envs to all envs
    all_envs.extend(prefix_envs.iter().map(|(name, value)| {
        (OsStr::new(name.as_str()).into(), OsStr::new(value.as_str()).into())
    }));

    Ok(SpawnExecution {
        all_envs: Arc::new(all_envs),
        resolved_cache_config,
        cwd: Arc::clone(&resolved_config.resolved_options.cwd),
        command_kind,
    })
}

/// Expand the parsed task request (like `run -r build`/`exec tsc`/`lint`) into an execution graph.
pub fn plan_query_request_as_execution_graph(
    query_task_request: QueryPlanRequest,
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

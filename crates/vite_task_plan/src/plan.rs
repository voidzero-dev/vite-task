use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    env::home_dir,
    ffi::OsStr,
    path::Path,
    sync::Arc,
};

use futures_util::FutureExt;
use vite_path::AbsolutePath;
use vite_shell::try_parse_as_and_list;
use vite_str::Str;
use vite_task_graph::{TaskNodeIndex, config::ResolvedTaskOptions};

use crate::{
    ExecutionItem, ExecutionItemKind, LeafExecutionKind, PlanContext, ResolvedCacheMetadata,
    SpawnCommandKind, SpawnExecution, TaskExecution,
    envs::ResolvedEnvs,
    error::{CdCommandError, Error, TaskPlanErrorKind, TaskPlanErrorKindResultExt},
    execution_graph::{ExecutionGraph, ExecutionNodeIndex},
    in_process::InProcessExecution,
    plan_request::{PlanRequest, QueryPlanRequest, SyntheticPlanRequest},
};

async fn plan_task_as_execution_node(
    task_node_index: TaskNodeIndex,
    mut context: PlanContext<'_>,
) -> Result<TaskExecution, Error> {
    // Check for recursions in the task call stack.
    context
        .check_recursion(task_node_index)
        .map_err(TaskPlanErrorKind::TaskRecursionDetected)
        .with_plan_context(&context)?;

    let task_node = &context.indexed_task_graph().task_graph()[task_node_index];
    let command_str = task_node.resolved_config.command.as_str();

    // Prepend {package_path}/node_modules/.bin to PATH
    let package_node_modules_bin_path = context
        .indexed_task_graph()
        .get_package_path_for_task(task_node_index)
        .join("node_modules")
        .join(".bin");
    if let Err(join_paths_error) = context.prepend_path(&package_node_modules_bin_path) {
        // Push the current task frame with full command span (the path was added for every and_item of the command) before returning the error
        context.push_stack_frame(task_node_index, 0..command_str.len());
        return Err(TaskPlanErrorKind::AddNodeModulesBinPathError { join_paths_error })
            .with_plan_context(&context);
    }

    let mut items = Vec::<ExecutionItem>::new();

    let mut cwd = Arc::clone(context.cwd());

    // TODO: variable expansion (https://crates.io/crates/shellexpand) BEFORE parsing
    // Try to parse the command string as a list of subcommands separated by `&&`
    if let Some(parsed_subcommands) = try_parse_as_and_list(command_str) {
        let and_item_count = parsed_subcommands.len();
        for (index, (and_item, add_item_span)) in parsed_subcommands.into_iter().enumerate() {
            // Duplicate the context before modifying it for each and_item
            let mut context = context.duplicate();
            context.push_stack_frame(task_node_index, add_item_span.clone());

            let mut args = and_item.args;
            let extra_args = if index == and_item_count - 1 {
                // For the last and_item, append extra args from the plan context
                Arc::clone(context.extra_args())
            } else {
                Arc::new([])
            };
            args.extend(extra_args.iter().cloned());

            // Handle `cd` builtin command
            if and_item.program == "cd" {
                let cd_target: Cow<'_, Path> = match args.as_slice() {
                    // No args, go to home directory
                    [] => home_dir()
                        .ok_or_else(|| {
                            TaskPlanErrorKind::CdCommandError(CdCommandError::NoHomeDirectory)
                        })
                        .with_plan_context(&context)?
                        .into(),
                    [dir] => Path::new(dir.as_str()).into(),
                    _ => {
                        return Err(TaskPlanErrorKind::CdCommandError(CdCommandError::ToManyArgs))
                            .with_plan_context(&context);
                    }
                };
                cwd = cwd.join(cd_target.as_ref()).into();
                continue;
            }

            // Check for builtin commands like `echo ...`
            if let Some(builtin_execution) =
                InProcessExecution::get_builtin_execution(&and_item.program, args.iter(), &cwd)
            {
                items.push(ExecutionItem {
                    command_span: add_item_span,
                    plan_cwd: Arc::clone(&cwd),
                    extra_args,
                    kind: ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(builtin_execution)),
                });
                continue;
            }

            // Try to parse the args of an and_item to a task request like `run -r build`
            let task_request = context
                .callbacks()
                .get_plan_request(&and_item.program, &args, &cwd)
                .await
                .map_err(|error| TaskPlanErrorKind::ParsePlanRequestError { error })
                .with_plan_context(&context)?;

            let execution_item_kind: ExecutionItemKind = match task_request {
                // Expand task query like `vite run -r build`
                Some(PlanRequest::Query(query_plan_request)) => {
                    // Add prefix envs to the context
                    context.add_envs(and_item.envs.iter());
                    let execution_graph = plan_query_request(query_plan_request, context).await?;
                    ExecutionItemKind::Expanded(execution_graph)
                }
                // Synthetic task, like `vite lint`
                Some(PlanRequest::Synthetic(synthetic_plan_request)) => {
                    let spawn_execution = plan_synthetic_request(
                        &and_item.envs,
                        synthetic_plan_request,
                        context.cwd(),
                        context.envs(),
                    )
                    .with_plan_context(&context)?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
                // Normal 3rd party tool command (like `tsc --noEmit`)
                None => {
                    let spawn_execution = plan_spawn_execution(
                        &and_item.envs,
                        SpawnCommandKind::Program {
                            program: OsStr::new(&and_item.program).into(),
                            args: args.into(),
                        },
                        &task_node.resolved_config.resolved_options,
                        context.envs(),
                    )
                    .with_plan_context(&context)?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
            };
            items.push(ExecutionItem {
                command_span: add_item_span,
                plan_cwd: Arc::clone(&cwd),
                extra_args,
                kind: execution_item_kind,
            });
        }
    } else {
        let spawn_execution = plan_spawn_execution(
            &BTreeMap::new(),
            SpawnCommandKind::ShellScript {
                script: command_str.into(),
                args: Arc::clone(context.extra_args()),
            },
            &task_node.resolved_config.resolved_options,
            context.envs(),
        )
        .with_plan_context(&context)?;
        items.push(ExecutionItem {
            command_span: 0..command_str.len(),
            plan_cwd: cwd,
            extra_args: Arc::clone(context.extra_args()),
            kind: ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution)),
        });
    }

    Ok(TaskExecution { task_node_index, items })
}

pub fn plan_synthetic_request(
    prefix_envs: &BTreeMap<Str, Str>,
    synthetic_plan_request: SyntheticPlanRequest,
    cwd: &Arc<AbsolutePath>,
    envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
) -> Result<SpawnExecution, TaskPlanErrorKind> {
    let SyntheticPlanRequest { program, args, task_options } = synthetic_plan_request;
    let resolved_options = ResolvedTaskOptions::resolve(task_options, &cwd);
    plan_spawn_execution(
        prefix_envs,
        SpawnCommandKind::Program { program, args },
        &resolved_options,
        envs,
    )
}

fn plan_spawn_execution(
    prefix_envs: &BTreeMap<Str, Str>,
    command_kind: SpawnCommandKind,
    resolved_task_options: &ResolvedTaskOptions,
    envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
) -> Result<SpawnExecution, TaskPlanErrorKind> {
    // all envs available in the current context
    let mut all_envs = envs.clone();

    let mut resolved_cache_metadata = None;
    if let Some(cache_config) = &resolved_task_options.cache_config {
        // Resolve envs according cache configs
        let mut resolved_envs = ResolvedEnvs::resolve(&mut all_envs, &cache_config.env_config)
            .map_err(TaskPlanErrorKind::ResolveEnvError)?;

        // Add prefix envs to fingerprinted envs
        resolved_envs
            .fingerprinted_envs
            .extend(prefix_envs.iter().map(|(name, value)| (name.clone(), value.as_str().into())));
        resolved_cache_metadata = Some(ResolvedCacheMetadata { resolved_envs });
    }

    // Add prefix envs to all envs
    all_envs.extend(prefix_envs.iter().map(|(name, value)| {
        (OsStr::new(name.as_str()).into(), OsStr::new(value.as_str()).into())
    }));

    Ok(SpawnExecution {
        all_envs: Arc::new(all_envs),
        resolved_cache_metadata,
        cwd: Arc::clone(&resolved_task_options.cwd),
        command_kind,
    })
}

/// Expand the parsed task request (like `run -r build`/`exec tsc`/`lint`) into an execution graph.
pub async fn plan_query_request(
    query_plan_request: QueryPlanRequest,
    mut context: PlanContext<'_>,
) -> Result<ExecutionGraph, Error> {
    context.set_extra_args(Arc::clone(&query_plan_request.plan_options.extra_args));
    // Query matching tasks from the task graph
    let task_node_index_graph = context
        .indexed_task_graph()
        .query_tasks(query_plan_request.query)
        .map_err(TaskPlanErrorKind::TaskQueryError)
        .with_plan_context(&context)?;

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
        let task_execution =
            plan_task_as_execution_node(task_index, context.duplicate()).boxed_local().await?;
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

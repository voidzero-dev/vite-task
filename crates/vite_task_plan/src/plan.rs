use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    env::home_dir,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use futures_util::FutureExt;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf, relative::InvalidPathDataError};
use vite_shell::try_parse_as_and_list;
use vite_str::Str;
use vite_task_graph::{TaskNodeIndex, config::ResolvedTaskOptions};

use crate::{
    ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, PlanContext,
    SpawnCommand, SpawnExecution, TaskExecution,
    cache_metadata::{CacheMetadata, ExecutionCacheKey, ProgramFingerprint, SpawnFingerprint},
    envs::EnvFingerprints,
    error::{
        CdCommandError, Error, PathFingerprintError, PathFingerprintErrorKind, PathType,
        TaskPlanErrorKind, TaskPlanErrorKindResultExt,
    },
    execution_graph::{ExecutionGraph, ExecutionNodeIndex},
    in_process::InProcessExecution,
    path_env::get_path_env,
    plan_request::{PlanRequest, QueryPlanRequest, ScriptCommand, SyntheticPlanRequest},
};

/// Locate the executable path for a given program name in the provided envs and cwd.
fn which(
    program: &Arc<OsStr>,
    envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
    cwd: &Arc<AbsolutePath>,
) -> Result<Arc<AbsolutePath>, crate::error::WhichError> {
    let path_env = get_path_env(envs);
    let executable_path = which::which_in(program, path_env, cwd.as_path()).map_err(|err| {
        crate::error::WhichError {
            program: Arc::clone(program),
            path_env: path_env.map(Arc::clone),
            cwd: Arc::clone(cwd),
            error: err,
        }
    })?;
    Ok(AbsolutePathBuf::new(executable_path)
        .expect("path returned by which::which_in should always be absolute")
        .into())
}

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

    let package_path = context.indexed_task_graph().get_package_path_for_task(task_node_index);
    // Prepend {package_path}/node_modules/.bin to PATH
    let package_node_modules_bin_path = package_path.join("node_modules").join(".bin");
    if let Err(join_paths_error) = context.prepend_path(&package_node_modules_bin_path) {
        // Push the current task frame with full command span (the path was added for every and_item of the command) before returning the error
        context.push_stack_frame(task_node_index, 0..command_str.len());
        return Err(TaskPlanErrorKind::AddNodeModulesBinPathError { join_paths_error })
            .with_plan_context(&context);
    }

    let mut items = Vec::<ExecutionItem>::new();

    // Use task's resolved cwd for display (from task config's cwd option)
    let mut cwd = Arc::clone(&task_node.resolved_config.resolved_options.cwd);

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

            // Build execution display
            let execution_item_display = ExecutionItemDisplay {
                command: {
                    let mut command = Str::from(&command_str[add_item_span.clone()]);
                    for arg in extra_args.iter() {
                        command.push(' ');
                        command.push_str(shell_escape::escape(arg.as_str().into()).as_ref());
                    }
                    command
                },
                and_item_index: if and_item_count > 1 { Some(index) } else { None },
                cwd: Arc::clone(&cwd),
                task_display: task_node.task_display.clone(),
            };

            // Check for builtin commands like `echo ...`
            if let Some(builtin_execution) =
                InProcessExecution::get_builtin_execution(&and_item.program, args.iter(), &cwd)
            {
                items.push(ExecutionItem {
                    execution_item_display,
                    kind: ExecutionItemKind::Leaf(LeafExecutionKind::InProcess(builtin_execution)),
                });
                continue;
            }

            // Create execution cache key for this and_item
            let task_execution_cache_key = ExecutionCacheKey::UserTask {
                task_name: task_node.task_display.task_name.clone(),
                and_item_index: index,
                extra_args: Arc::clone(&extra_args),
                package_path: strip_prefix_for_cache(package_path, context.workspace_path())
                    .map_err(|kind| {
                        TaskPlanErrorKind::PathFingerprintError(PathFingerprintError {
                            kind,
                            path_type: PathType::PackagePath,
                        })
                    })
                    .with_plan_context(&context)?,
            };

            // Try to parse the args of an and_item to a plan request like `run -r build`
            let envs: Arc<HashMap<Arc<OsStr>, Arc<OsStr>>> = context.envs().clone().into();
            let mut script_command = ScriptCommand {
                program: and_item.program.clone(),
                args: args.into(),
                envs,
                cwd: Arc::clone(&cwd),
            };
            let plan_request = context
                .callbacks()
                .get_plan_request(&mut script_command)
                .await
                .map_err(|error| TaskPlanErrorKind::ParsePlanRequestError {
                    program: script_command.program.clone(),
                    args: Arc::clone(&script_command.args),
                    cwd: Arc::clone(&script_command.cwd),
                    error,
                })
                .with_plan_context(&context)?;

            let execution_item_kind: ExecutionItemKind = match plan_request {
                // Expand task query like `vite run -r build`
                Some(PlanRequest::Query(query_plan_request)) => {
                    // Add prefix envs to the context
                    context.add_envs(and_item.envs.iter());
                    let execution_graph = plan_query_request(query_plan_request, context).await?;
                    ExecutionItemKind::Expanded(execution_graph)
                }
                // Synthetic task (from CommandHandler)
                Some(PlanRequest::Synthetic(synthetic_plan_request)) => {
                    let spawn_execution = plan_synthetic_request(
                        context.workspace_path(),
                        &and_item.envs,
                        synthetic_plan_request,
                        Some(task_execution_cache_key),
                        &cwd,
                    )
                    .with_plan_context(&context)?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
                // Normal 3rd party tool command (like `tsc --noEmit`), using potentially mutated script_command
                None => {
                    let program_path = which(
                        &OsStr::new(&script_command.program).into(),
                        &script_command.envs,
                        &script_command.cwd,
                    )
                    .map_err(TaskPlanErrorKind::ProgramNotFound)
                    .with_plan_context(&context)?;
                    let spawn_execution = plan_spawn_execution(
                        context.workspace_path(),
                        Some(task_execution_cache_key),
                        &and_item.envs,
                        &task_node.resolved_config.resolved_options,
                        &script_command.envs,
                        program_path,
                        script_command.args,
                    )
                    .with_plan_context(&context)?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
            };
            items.push(ExecutionItem { execution_item_display, kind: execution_item_kind });
        }
    } else {
        let mut context = context.duplicate();
        context.push_stack_frame(task_node_index, 0..command_str.len());

        let execution_item_display = ExecutionItemDisplay {
            command: command_str.into(),
            and_item_index: None,
            cwd,
            task_display: task_node.task_display.clone(),
        };

        static SHELL_PROGRAM_PATH: LazyLock<Arc<AbsolutePath>> = LazyLock::new(|| {
            if cfg!(target_os = "windows") {
                AbsolutePathBuf::new(
                    which::which("cmd.exe")
                        .unwrap_or_else(|_| PathBuf::from("C:\\Windows\\System32\\cmd.exe")),
                )
                .unwrap()
                .into()
            } else {
                AbsolutePath::new("/bin/sh").unwrap().into()
            }
        });

        static SHELL_ARGS: &[&str] =
            if cfg!(target_os = "windows") { &["/d", "/s", "/c"] } else { &["-c"] };

        let mut script = Str::from(command_str);
        for arg in context.extra_args().iter() {
            script.push(' ');
            script.push_str(shell_escape::escape(arg.as_str().into()).as_ref());
        }

        let spawn_execution = plan_spawn_execution(
            context.workspace_path(),
            Some(ExecutionCacheKey::UserTask {
                task_name: task_node.task_display.task_name.clone(),
                and_item_index: 0,
                extra_args: Arc::clone(context.extra_args()),
                package_path: strip_prefix_for_cache(package_path, context.workspace_path())
                    .map_err(|kind| {
                        TaskPlanErrorKind::PathFingerprintError(PathFingerprintError {
                            kind,
                            path_type: PathType::PackagePath,
                        })
                    })
                    .with_plan_context(&context)?,
            }),
            &BTreeMap::new(),
            &task_node.resolved_config.resolved_options,
            context.envs(),
            Arc::clone(&*SHELL_PROGRAM_PATH),
            Arc::from_iter(SHELL_ARGS.iter().map(|s| Str::from(*s)).chain(std::iter::once(script))),
        )
        .with_plan_context(&context)?;
        items.push(ExecutionItem {
            execution_item_display,
            kind: ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution)),
        });
    }

    Ok(TaskExecution { task_display: task_node.task_display.clone(), items })
}

pub fn plan_synthetic_request(
    workspace_path: &Arc<AbsolutePath>,
    prefix_envs: &BTreeMap<Str, Str>,
    synthetic_plan_request: SyntheticPlanRequest,
    execution_cache_key: Option<ExecutionCacheKey>,
    cwd: &Arc<AbsolutePath>,
) -> Result<SpawnExecution, TaskPlanErrorKind> {
    let SyntheticPlanRequest { program, args, task_options, envs } = synthetic_plan_request;

    let program_path = which(&program, &envs, cwd).map_err(TaskPlanErrorKind::ProgramNotFound)?;
    let resolved_options = ResolvedTaskOptions::resolve(task_options, &cwd);

    plan_spawn_execution(
        workspace_path,
        execution_cache_key,
        prefix_envs,
        &resolved_options,
        &envs,
        program_path,
        args,
    )
}

fn strip_prefix_for_cache(
    path: &Arc<AbsolutePath>,
    workspace_path: &Arc<AbsolutePath>,
) -> Result<RelativePathBuf, PathFingerprintErrorKind> {
    match path.strip_prefix(&*workspace_path) {
        Ok(Some(rel_path)) => Ok(rel_path),
        Ok(None) => Err(PathFingerprintErrorKind::PathOutsideWorkspace {
            path: Arc::clone(path),
            workspace_path: Arc::clone(workspace_path),
        }),
        Err(err) => Err(PathFingerprintErrorKind::NonPortableRelativePath {
            path: err.stripped_path.into(),
            error: err.invalid_path_data_error,
        }),
    }
}

fn plan_spawn_execution(
    workspace_path: &Arc<AbsolutePath>,
    execution_cache_key: Option<ExecutionCacheKey>,
    prefix_envs: &BTreeMap<Str, Str>,
    resolved_task_options: &ResolvedTaskOptions,
    envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
    program_path: Arc<AbsolutePath>,
    args: Arc<[Str]>,
) -> Result<SpawnExecution, TaskPlanErrorKind> {
    // all envs available in the current context
    let mut all_envs = envs.clone();
    let cwd = Arc::clone(&resolved_task_options.cwd);

    let mut resolved_cache_metadata = None;
    if let Some(cache_config) = &resolved_task_options.cache_config {
        // Resolve envs according cache configs
        let mut env_fingerprints =
            EnvFingerprints::resolve(&mut all_envs, &cache_config.env_config)
                .map_err(TaskPlanErrorKind::ResolveEnvError)?;

        // Add prefix envs to fingerprinted envs
        env_fingerprints
            .fingerprinted_envs
            .extend(prefix_envs.iter().map(|(name, value)| (name.clone(), value.as_str().into())));

        let program_fingerprint = match strip_prefix_for_cache(&program_path, workspace_path) {
            Ok(relative_program_path) => {
                ProgramFingerprint::InsideWorkspace { relative_program_path }
            }
            Err(PathFingerprintErrorKind::PathOutsideWorkspace { path, .. }) => {
                let program_name_os_str = path.as_path().file_name().unwrap_or_default();
                let Some(program_name_str) = program_name_os_str.to_str() else {
                    return Err(PathFingerprintError {
                        kind: PathFingerprintErrorKind::NonPortableRelativePath {
                            path: Path::new(program_name_os_str).into(),
                            error: InvalidPathDataError::NonUtf8,
                        },
                        path_type: PathType::Program,
                    }
                    .into());
                };
                ProgramFingerprint::OutsideWorkspace { program_name: program_name_str.into() }
            }
            Err(err) => {
                return Err(PathFingerprintError { kind: err, path_type: PathType::Program }.into());
            }
        };

        let spawn_fingerprint: SpawnFingerprint = SpawnFingerprint {
            cwd: strip_prefix_for_cache(&cwd, workspace_path)
                .map_err(|kind| PathFingerprintError { kind, path_type: PathType::Cwd })?,
            program_fingerprint,
            args: Arc::clone(&args),
            env_fingerprints,
            fingerprint_ignores: None,
        };
        if let Some(execution_cache_key) = execution_cache_key {
            resolved_cache_metadata =
                Some(CacheMetadata { execution_cache_key, spawn_fingerprint });
        }
    }

    // Add prefix envs to all envs
    all_envs.extend(prefix_envs.iter().map(|(name, value)| {
        (OsStr::new(name.as_str()).into(), OsStr::new(value.as_str()).into())
    }));

    Ok(SpawnExecution {
        spawn_command: SpawnCommand {
            program_path,
            args: Arc::clone(&args),
            cwd,
            all_envs: Arc::new(all_envs.into_iter().collect()),
        },
        cache_metadata: resolved_cache_metadata,
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

#[expect(
    clippy::disallowed_types,
    reason = "Path is needed for cd command argument and error reporting"
)]
use std::path::Path;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    env::home_dir,
    ffi::OsStr,
    sync::{Arc, LazyLock},
};

use futures_util::FutureExt;
use rustc_hash::FxHashMap;
use vite_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf, relative::InvalidPathDataError};
use vite_shell::try_parse_as_and_list;
use vite_str::Str;
use vite_task_graph::{
    TaskNodeIndex,
    config::{
        CacheConfig, ResolvedTaskOptions,
        user::{UserCacheConfig, UserTaskOptions},
    },
};

use crate::{
    ExecutionItem, ExecutionItemDisplay, ExecutionItemKind, LeafExecutionKind, PlanContext,
    SpawnCommand, SpawnExecution, TaskExecution,
    cache_metadata::{CacheMetadata, ExecutionCacheKey, ProgramFingerprint, SpawnFingerprint},
    envs::EnvFingerprints,
    error::{CdCommandError, Error, PathFingerprintError, PathFingerprintErrorKind, PathType},
    execution_graph::{ExecutionGraph, ExecutionNodeIndex, InnerExecutionGraph},
    in_process::InProcessExecution,
    path_env::get_path_env,
    plan_request::{PlanRequest, QueryPlanRequest, ScriptCommand, SyntheticPlanRequest},
};

/// Locate the executable path for a given program name in the provided envs and cwd.
fn which(
    program: &Arc<OsStr>,
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
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

#[expect(clippy::too_many_lines, reason = "sequential planning steps are clearer in one function")]
#[expect(clippy::future_not_send, reason = "PlanContext contains !Send dyn PlanRequestParser")]
async fn plan_task_as_execution_node(
    task_node_index: TaskNodeIndex,
    mut context: PlanContext<'_>,
) -> Result<TaskExecution, Error> {
    // Check for recursions in the task call stack.
    context.check_recursion(task_node_index)?;

    let task_node = &context.indexed_task_graph().task_graph()[task_node_index];
    let command_str = task_node.resolved_config.command.as_str();

    let package_path = context.indexed_task_graph().get_package_path_for_task(task_node_index);
    // Prepend {package_path}/node_modules/.bin to PATH
    let package_node_modules_bin_path = package_path.join("node_modules").join(".bin");
    if let Err(join_paths_error) = context.prepend_path(&package_node_modules_bin_path) {
        return Err(Error::AddNodeModulesBinPath { join_paths_error });
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
                #[expect(
                    clippy::disallowed_types,
                    reason = "Path is needed for std::env::home_dir return type and AbsolutePath::join"
                )]
                let cd_target: Cow<'_, Path> = match args.as_slice() {
                    // No args, go to home directory
                    [] => {
                        home_dir().ok_or(Error::CdCommand(CdCommandError::NoHomeDirectory))?.into()
                    }
                    [dir] => Path::new(dir.as_str()).into(),
                    _ => {
                        return Err(Error::CdCommand(CdCommandError::ToManyArgs));
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
                    .map_err(|kind| PathFingerprintError {
                        kind,
                        path_type: PathType::PackagePath,
                    })?,
            };

            // Try to parse the args of an and_item to a plan request like `run -r build`
            let envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>> = context.envs().clone().into();
            let mut script_command = ScriptCommand {
                program: and_item.program.clone(),
                args: args.into(),
                envs,
                cwd: Arc::clone(&cwd),
            };
            let plan_request =
                context.callbacks().get_plan_request(&mut script_command).await.map_err(
                    |error| Error::ParsePlanRequest {
                        program: script_command.program.clone(),
                        args: Arc::clone(&script_command.args),
                        cwd: Arc::clone(&script_command.cwd),
                        error,
                    },
                )?;

            let execution_item_kind: ExecutionItemKind = match plan_request {
                // Expand task query like `vp run -r build`
                Some(PlanRequest::Query(query_plan_request)) => {
                    // Save task name before consuming the request
                    let task_name = query_plan_request.query.task_name.clone();
                    // Add prefix envs to the context
                    context.add_envs(and_item.envs.iter());
                    let execution_graph = plan_query_request(query_plan_request, context)
                        .await
                        .map_err(|error| Error::NestPlan {
                            task_display: task_node.task_display.clone(),
                            command: Str::from(&command_str[add_item_span.clone()]),
                            error: Box::new(error),
                        })?;
                    // An empty execution graph means no tasks matched the query.
                    // At the top level the session shows the task selector UI,
                    // but in a nested context there is no UI — propagate as an error.
                    if execution_graph.node_count() == 0 {
                        return Err(Error::NestPlan {
                            task_display: task_node.task_display.clone(),
                            command: Str::from(&command_str[add_item_span]),
                            error: Box::new(Error::NoTasksMatched(task_name)),
                        });
                    }
                    ExecutionItemKind::Expanded(execution_graph)
                }
                // Synthetic task (from CommandHandler)
                Some(PlanRequest::Synthetic(synthetic_plan_request)) => {
                    let parent_cache_config = task_node
                        .resolved_config
                        .resolved_options
                        .cache_config
                        .as_ref()
                        .map_or(ParentCacheConfig::Disabled, |config| {
                            ParentCacheConfig::Inherited(config.clone())
                        });
                    let spawn_execution = plan_synthetic_request(
                        context.workspace_path(),
                        &and_item.envs,
                        synthetic_plan_request,
                        Some(task_execution_cache_key),
                        &cwd,
                        parent_cache_config,
                    )?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
                // Normal 3rd party tool command (like `tsc --noEmit`), using potentially mutated script_command
                None => {
                    let program_path = which(
                        &OsStr::new(&script_command.program).into(),
                        &script_command.envs,
                        &script_command.cwd,
                    )?;
                    let spawn_execution = plan_spawn_execution(
                        context.workspace_path(),
                        Some(task_execution_cache_key),
                        &and_item.envs,
                        &task_node.resolved_config.resolved_options,
                        &script_command.envs,
                        program_path,
                        script_command.args,
                    )?;
                    ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution))
                }
            };
            items.push(ExecutionItem { execution_item_display, kind: execution_item_kind });
        }
    } else {
        #[expect(clippy::disallowed_types, reason = "PathBuf needed for which fallback path")]
        static SHELL_PROGRAM_PATH: LazyLock<Arc<AbsolutePath>> =
            LazyLock::new(|| {
                if cfg!(target_os = "windows") {
                    AbsolutePathBuf::new(which::which("cmd.exe").unwrap_or_else(|_| {
                        std::path::PathBuf::from("C:\\Windows\\System32\\cmd.exe")
                    }))
                    .unwrap()
                    .into()
                } else {
                    AbsolutePath::new("/bin/sh").unwrap().into()
                }
            });

        static SHELL_ARGS: &[&str] =
            if cfg!(target_os = "windows") { &["/d", "/s", "/c"] } else { &["-c"] };

        let mut context = context.duplicate();
        context.push_stack_frame(task_node_index, 0..command_str.len());

        let execution_item_display = ExecutionItemDisplay {
            command: command_str.into(),
            and_item_index: None,
            cwd,
            task_display: task_node.task_display.clone(),
        };

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
                    .map_err(|kind| PathFingerprintError {
                        kind,
                        path_type: PathType::PackagePath,
                    })?,
            }),
            &BTreeMap::new(),
            &task_node.resolved_config.resolved_options,
            context.envs(),
            Arc::clone(&*SHELL_PROGRAM_PATH),
            SHELL_ARGS.iter().map(|s| Str::from(*s)).chain(std::iter::once(script)).collect(),
        )?;
        items.push(ExecutionItem {
            execution_item_display,
            kind: ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(spawn_execution)),
        });
    }

    Ok(TaskExecution { task_display: task_node.task_display.clone(), items })
}

/// Cache configuration inherited from the parent task that contains a synthetic command.
///
/// When a synthetic task (e.g., `vp lint` expanding to `oxlint`) appears inside a
/// user-defined task's script, the parent task's cache configuration should constrain
/// the synthetic task's caching behavior.
pub enum ParentCacheConfig {
    /// No parent task (top-level synthetic command like `vp lint` run directly).
    /// The synthetic task uses its own default cache configuration.
    None,

    /// Parent task has caching disabled (`cache: false` or `cacheScripts` not enabled).
    /// The synthetic task should also have caching disabled.
    Disabled,

    /// Parent task has caching enabled with this configuration.
    /// The synthetic task inherits this config, merged with its own additions.
    Inherited(CacheConfig),
}

/// Resolves the effective cache configuration for a synthetic task by combining
/// the parent task's cache config with the synthetic command's own additions.
///
/// Synthetic tasks (e.g., `vp lint` → `oxlint`) may declare their own cache-related
/// env requirements (e.g., `pass_through_envs` for env-test). When a parent task
/// exists, its cache config takes precedence:
/// - If the parent disables caching, the synthetic task is also uncached.
/// - If the parent enables caching but the synthetic disables it, caching is disabled.
/// - If both parent and synthetic enable caching, the synthetic inherits the parent's
///   env config and merges in any additional envs the synthetic command needs.
/// - If there is no parent (top-level invocation), the synthetic task's own
///   [`UserCacheConfig`] is resolved with defaults.
fn resolve_synthetic_cache_config(
    parent: ParentCacheConfig,
    synthetic_cache_config: UserCacheConfig,
    cwd: &Arc<AbsolutePath>,
) -> Option<CacheConfig> {
    match parent {
        ParentCacheConfig::None => {
            // Top-level: resolve from synthetic's own config
            ResolvedTaskOptions::resolve(
                UserTaskOptions {
                    cache_config: synthetic_cache_config,
                    cwd_relative_to_package: None,
                    depends_on: None,
                },
                cwd,
            )
            .cache_config
        }
        ParentCacheConfig::Disabled => Option::None,
        ParentCacheConfig::Inherited(mut parent_config) => {
            // Cache is enabled only if both parent and synthetic want it.
            // Merge synthetic's additions into parent's config.
            match synthetic_cache_config {
                UserCacheConfig::Disabled { .. } => Option::None,
                UserCacheConfig::Enabled { enabled_cache_config, .. } => {
                    if let Some(extra_envs) = enabled_cache_config.envs {
                        parent_config.env_config.fingerprinted_envs.extend(extra_envs.into_vec());
                    }
                    if let Some(extra_pts) = enabled_cache_config.pass_through_envs {
                        parent_config.env_config.pass_through_envs.extend(extra_pts);
                    }
                    Some(parent_config)
                }
            }
        }
    }
}

#[expect(clippy::result_large_err, reason = "Error is large for diagnostics")]
pub fn plan_synthetic_request(
    workspace_path: &Arc<AbsolutePath>,
    prefix_envs: &BTreeMap<Str, Str>,
    synthetic_plan_request: SyntheticPlanRequest,
    execution_cache_key: Option<ExecutionCacheKey>,
    cwd: &Arc<AbsolutePath>,
    parent_cache_config: ParentCacheConfig,
) -> Result<SpawnExecution, Error> {
    let SyntheticPlanRequest { program, args, cache_config, envs } = synthetic_plan_request;

    let program_path = which(&program, &envs, cwd)?;
    let resolved_cache_config =
        resolve_synthetic_cache_config(parent_cache_config, cache_config, cwd);
    let resolved_options =
        ResolvedTaskOptions { cwd: Arc::clone(cwd), cache_config: resolved_cache_config };

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
    match path.strip_prefix(workspace_path) {
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

#[expect(clippy::result_large_err, reason = "Error is large for diagnostics")]
#[expect(
    clippy::needless_pass_by_value,
    reason = "program_path ownership is needed for Arc construction"
)]
fn plan_spawn_execution(
    workspace_path: &Arc<AbsolutePath>,
    execution_cache_key: Option<ExecutionCacheKey>,
    prefix_envs: &BTreeMap<Str, Str>,
    resolved_task_options: &ResolvedTaskOptions,
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    program_path: Arc<AbsolutePath>,
    args: Arc<[Str]>,
) -> Result<SpawnExecution, Error> {
    // all envs available in the current context
    let mut all_envs = envs.clone();
    let cwd = Arc::clone(&resolved_task_options.cwd);

    let mut resolved_cache_metadata = None;
    if let Some(cache_config) = &resolved_task_options.cache_config {
        // Resolve envs according cache configs
        let mut env_fingerprints =
            EnvFingerprints::resolve(&mut all_envs, &cache_config.env_config)
                .map_err(Error::ResolveEnv)?;

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
                #[expect(
                    clippy::manual_let_else,
                    reason = "? operator doesn't apply since early return has a different error type"
                )]
                let program_name_str = match program_name_os_str.to_str() {
                    Some(s) => s,
                    None => {
                        #[expect(
                            clippy::disallowed_types,
                            reason = "Arc<Path> for non-UTF-8 path data in error"
                        )]
                        return Err(PathFingerprintError {
                            kind: PathFingerprintErrorKind::NonPortableRelativePath {
                                path: Path::new(program_name_os_str).into(),
                                error: InvalidPathDataError::NonUtf8,
                            },
                            path_type: PathType::Program,
                        }
                        .into());
                    }
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
                Some(CacheMetadata { spawn_fingerprint, execution_cache_key });
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

/// Expand the parsed task request (like `run -r build`/`lint`) into an execution graph.
///
/// Builds a `DiGraph` of task executions, then validates it is acyclic via
/// `ExecutionGraph::try_from_graph`. Returns `CycleDependencyDetected` if a cycle is found.
#[expect(clippy::future_not_send, reason = "PlanContext contains !Send dyn PlanRequestParser")]
pub async fn plan_query_request(
    query_plan_request: QueryPlanRequest,
    mut context: PlanContext<'_>,
) -> Result<ExecutionGraph, Error> {
    context.set_extra_args(Arc::clone(&query_plan_request.plan_options.extra_args));
    // Query matching tasks from the task graph.
    // An empty graph means no tasks matched; the caller (session) handles
    // empty graphs by showing the task selector.
    let task_query_result = context.indexed_task_graph().query_tasks(&query_plan_request.query)?;

    #[expect(clippy::print_stderr, reason = "user-facing warning for typos in --filter")]
    for selector in &task_query_result.unmatched_selectors {
        eprintln!("No packages matched the filter: {selector}");
    }

    let task_node_index_graph = task_query_result.execution_graph;

    let mut execution_node_indices_by_task_index =
        FxHashMap::<TaskNodeIndex, ExecutionNodeIndex>::with_capacity_and_hasher(
            task_node_index_graph.node_count(),
            rustc_hash::FxBuildHasher,
        );

    // Build the inner DiGraph first, then validate acyclicity at the end.
    let mut inner_graph = InnerExecutionGraph::with_capacity(
        task_node_index_graph.node_count(),
        task_node_index_graph.edge_count(),
    );

    // Plan each task node as execution nodes
    for task_index in task_node_index_graph.nodes() {
        let task_execution =
            plan_task_as_execution_node(task_index, context.duplicate()).boxed_local().await?;
        execution_node_indices_by_task_index
            .insert(task_index, inner_graph.add_node(task_execution));
    }

    // Add edges between execution nodes according to task dependencies
    for (from_task_index, to_task_index, ()) in task_node_index_graph.all_edges() {
        inner_graph.add_edge(
            execution_node_indices_by_task_index[&from_task_index],
            execution_node_indices_by_task_index[&to_task_index],
            (),
        );
    }

    // Validate the graph is acyclic.
    // `try_from_graph` performs a DFS; if a cycle is found, it returns
    // `CycleError` containing the full cycle path as node indices.
    ExecutionGraph::try_from_graph(inner_graph).map_err(|cycle| {
        // Map each execution node index in the cycle path to its human-readable TaskDisplay.
        // Every node in the cycle was added via `inner_graph.add_node()` above,
        // with a corresponding entry in `execution_node_indices_by_task_index`.
        let displays = cycle
            .cycle_path()
            .iter()
            .map(|&exec_idx| {
                execution_node_indices_by_task_index
                    .iter()
                    .find_map(|(task_idx, &mapped_exec_idx)| {
                        if mapped_exec_idx == exec_idx {
                            Some(context.indexed_task_graph().display_task(*task_idx))
                        } else {
                            None
                        }
                    })
                    .expect("cycle node must exist in execution_node_indices_by_task_index")
            })
            .collect();
        Error::CycleDependencyDetected(displays)
    })
}

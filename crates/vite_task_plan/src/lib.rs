mod context;
mod envs;
mod error;
pub mod execution_graph;
mod in_process;
mod path_env;
mod plan;
pub mod plan_request;

use std::{collections::HashMap, ffi::OsStr, fmt::Debug, ops::Range, sync::Arc};

use context::PlanContext;
use envs::ResolvedEnvs;
use error::{Error, TaskPlanErrorKind, TaskPlanErrorKindResultExt};
use execution_graph::ExecutionGraph;
use futures_core::future::BoxFuture;
use in_process::InProcessExecution;
use plan::{plan_query_request, plan_synthetic_request};
use plan_request::PlanRequest;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskGraphLoadError, TaskNodeIndex, query::TaskQuery};

/// Resolved cache configuration for a spawn execution.
#[derive(Debug)]
pub struct ResolvedCacheConfig {
    /// Environment variables that are used for fingerprinting the cache.
    pub resolved_envs: ResolvedEnvs,
}

/// A resolved spawn execution.
/// Unlike tasks in `vite_task_graph`, this struct contains all information needed for execution,
/// like resolved environment variables, current working directory, and additional args from cli.
#[derive(Debug)]
pub struct SpawnExecution {
    /// Resolved cache configuration for this execution. `None` means caching is disabled.
    pub resolved_cache_config: Option<ResolvedCacheConfig>,

    /// Environment variables to set for the command, including both fingerprinted and pass-through envs.
    pub all_envs: Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,

    /// Current working directory
    pub cwd: Arc<AbsolutePath>,

    /// parsed program with args or shell script
    pub command_kind: SpawnCommandKind,
}

/// The kind of a spawn command
#[derive(Debug)]
pub enum SpawnCommandKind {
    /// A program with args to be executed directly
    Program { program: Str, args: Arc<[Str]> },
    /// A script to be executed by os shell (sh or cmd)
    ShellScript(Str),
}

/// Represents how a task should be executed. It's the node type for the execution graph. Each node corresponds to a task.
#[derive(Debug)]
pub struct TaskExecution {
    /// The task index in the task graph
    pub task_node_index: TaskNodeIndex,

    /// A task's command is split by `&&` and expanded into multiple execution items.
    ///
    /// It contains a single item if the command has no `&&`
    pub items: Vec<ExecutionItem>,
}

/// An execution item, either expanded from a known vite subcommand, or a spawn execution.
#[derive(Debug)]
pub struct ExecutionItem {
    /// The range of the task command that this execution item is resolved from.
    ///
    /// This field is for displaying purpose only.
    /// The actual execution info (if this is spawn) is in `SpawnExecutionItem.command_kind`.
    pub command_span: Range<usize>,

    /// The kind of this execution item
    pub kind: ExecutionItemKind,
}

/// The kind of a leaf execution item, which cannot be expanded further.
#[derive(Debug)]
pub enum LeafExecutionKind {
    /// The execution is a spawn of a child process
    Spawn(SpawnExecution),
    /// The execution is done in-process by InProcessExecution::execute()
    InProcess(InProcessExecution),
}

/// An execution item, from a split subcommand in a task's command (`item1 && item2 && ...`).
#[derive(Debug)]
pub enum ExecutionItemKind {
    /// Expanded from a known vite subcommand, like `vite run ...` or `vite lint`.
    Expanded(ExecutionGraph),
    /// A normal execution that spawns a child process, like `tsc --noEmit`.
    Leaf(LeafExecutionKind),
}

/// Callbackes needed during planning.
/// See each method for details.
pub trait PlanCallbacks: Debug {
    fn load_task_graph(
        &mut self,
        cwd: &AbsolutePath,
    ) -> BoxFuture<'_, Result<Arc<vite_task_graph::IndexedTaskGraph>, TaskGraphLoadError>>;

    /// This is called for every parsable command in the task graph in order to determine how to execute it.
    ///
    /// `vite_task_plan` doesn't have the knowledge of how cli args should be parsed. It relies on this callback.
    ///
    /// - If it returns `Err`, the planning will abort with the returned error.
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(ParsedArgs::TaskQuery)`, the command will be expanded as a `ExpandedExecution` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(ParsedArgs::Synthetic)`, the command will become a `SpawnExecution` with the synthetic task.
    fn get_plan_request(
        &self,
        program: &str,
        args: &[Str],
    ) -> BoxFuture<'_, anyhow::Result<Option<PlanRequest>>>;
}

#[derive(Debug)]
pub struct ExecutionPlan {
    root_node: ExecutionItemKind,
}

pub struct Args {
    pub query: TaskQuery,
}

impl ExecutionPlan {
    pub fn root_node(&self) -> &ExecutionItemKind {
        &self.root_node
    }

    pub async fn plan(
        plan_request: PlanRequest,
        cwd: &Arc<AbsolutePath>,
        envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
        callbacks: &mut (dyn PlanCallbacks + '_),
    ) -> Result<Self, Error> {
        let root_node = match plan_request {
            PlanRequest::Query(query_plan_request) => {
                let indexed_task_graph = callbacks
                    .load_task_graph(cwd)
                    .await
                    .map_err(|load_error| TaskPlanErrorKind::TaskGraphLoadError(load_error))
                    .with_empty_call_stack()?;

                let context = PlanContext {
                    cwd: Arc::clone(cwd),
                    envs: envs.clone(),
                    callbacks,
                    task_call_stack: Vec::new(),
                    indexed_task_graph: &indexed_task_graph,
                };

                let execution_graph = plan_query_request(query_plan_request, context).await?;
                ExecutionItemKind::Expanded(execution_graph)
            }
            PlanRequest::Synthetic(synthetic_plan_request) => {
                let execution =
                    plan_synthetic_request(&Default::default(), synthetic_plan_request, cwd, envs)
                        .with_empty_call_stack()?;

                ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(execution))
            }
        };
        Ok(Self { root_node })
    }
}

pub mod cache_metadata;
mod context;
mod envs;
mod error;
pub mod execution_graph;
mod in_process;
mod path_env;
mod plan;
pub mod plan_request;

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fmt::Debug,
    ops::Range,
    sync::Arc,
};

use context::PlanContext;
use error::TaskPlanErrorKindResultExt;
pub use error::{Error, TaskPlanErrorKind};
use execution_graph::ExecutionGraph;
use in_process::InProcessExecution;
pub use path_env::get_path_env;
use plan::{plan_query_request, plan_synthetic_request};
use plan_request::PlanRequest;
use serde::{Serialize, ser::SerializeMap as _};
use vite_graph_ser::serialize_by_key;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskGraphLoadError, TaskNodeIndex, display::TaskDisplay};

use crate::path_env::prepend_path_env;

/// A resolved spawn execution.
/// Unlike tasks in `vite_task_graph`, this struct contains all information needed for execution,
/// like resolved environment variables, current working directory, and additional args from cli.
#[derive(Debug, Serialize)]
pub struct SpawnExecution {
    /// Cache metadata for this execution. `None` means caching is disabled.
    pub cache_metadata: Option<cache_metadata::CacheMetadata>,

    /// All information about a command to be spawned
    pub spawn_command: SpawnCommand,
}

/// All information about a command to be spawned.
#[derive(Debug, Serialize)]
pub struct SpawnCommand {
    /// A program with args to be executed directly
    pub program_path: Arc<AbsolutePath>,

    /// args to be passed to the program
    pub args: Arc<[Str]>,

    /// Environment variables to set for the command, including both fingerprinted and pass-through envs.
    #[serde(serialize_with = "serialize_envs")]
    pub all_envs: Arc<BTreeMap<Arc<OsStr>, Arc<OsStr>>>,

    /// Current working directory
    pub cwd: Arc<AbsolutePath>,
}

/// Serialize environment variables as a map from string to string for better readability.
fn serialize_envs<S>(
    envs: &BTreeMap<Arc<OsStr>, Arc<OsStr>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut map_ser = serializer.serialize_map(Some(envs.len()))?;
    for (key, value) in envs {
        map_ser.serialize_entry(&key.display().to_string(), &value.display().to_string())?;
    }
    map_ser.end()
}

/// Represents how a task should be executed. It's the node type for the execution graph. Each node corresponds to a task.
#[derive(Debug, Serialize)]
pub struct TaskExecution {
    /// The task this execution corresponds to
    pub task_display: TaskDisplay,

    /// A task's command is split by `&&` and expanded into multiple execution items.
    ///
    /// It contains a single item if the command has no `&&`
    pub items: Vec<ExecutionItem>,
}

impl vite_graph_ser::GetKey for TaskExecution {
    type Key<'a> = (&'a AbsolutePath, &'a str);

    fn key(&self) -> Result<Self::Key<'_>, String> {
        Ok((&self.task_display.package_path, &self.task_display.task_name))
    }
}

/// An execution item, either expanded from a known vite subcommand, or a spawn execution.
#[derive(Debug, Serialize)]
pub struct ExecutionItem {
    /// The range of the task command that this execution item is resolved from.
    ///
    /// This field is for displaying purpose only.
    /// The actual execution info (if this is spawn) is in `SpawnExecutionItem.command_kind`.
    pub command_span: Range<usize>,

    /// Extra args appended to this execution item from the cli (`vite run task [extra_args...]`).
    /// This is for computing the cache key along with the associated task.
    ///
    /// `kind` already contains the full resolved args for execution. No need to append these again.
    pub extra_args: Arc<[Str]>,

    /// The cwd when this execution item is planned.
    /// This is for displaying purpose only.
    ///
    /// `SpawnExecution.cwd` contains the actual cwd for execution.
    /// These two may differ if the task synthesizer returns a task with a different cwd.
    ///
    /// Hypothetically , if `vite lint-src` under cwd `packages/lib` synthesizes a task spawning `oxlint` under `packages/lib/src`.
    /// The spawned process' cwd will be `packages/lib/src`, while this field will be `packages/lib`,
    /// which will be displayed like `packages/lib$ vite lint-src``.
    pub plan_cwd: Arc<AbsolutePath>,

    /// The kind of this execution item
    pub kind: ExecutionItemKind,
}

/// The kind of a leaf execution item, which cannot be expanded further.
#[derive(Debug, Serialize)]
pub enum LeafExecutionKind {
    /// The execution is a spawn of a child process
    Spawn(SpawnExecution),
    /// The execution is done in-process by InProcessExecution::execute()
    InProcess(InProcessExecution),
}

/// An execution item, from a split subcommand in a task's command (`item1 && item2 && ...`).
#[derive(Debug, Serialize)]
pub enum ExecutionItemKind {
    /// Expanded from a known vite subcommand, like `vite run ...` or `vite lint`.
    Expanded(#[serde(serialize_with = "serialize_by_key")] ExecutionGraph),
    /// A normal execution that spawns a child process, like `tsc --noEmit`.
    Leaf(LeafExecutionKind),
}

/// The callback trait for parsing plan requests from cli args.
/// See the method for details.
#[async_trait::async_trait(?Send)]
pub trait PlanRequestParser: Debug {
    /// This is called for every parsable command in the task graph in order to determine how to execute it.
    ///
    /// `vite_task_plan` doesn't have the knowledge of how cli args should be parsed. It relies on this callback.
    ///
    /// - If it returns `Err`, the planning will abort with the returned error.
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(PlanRequest::Query)`, the command will be expanded as a `ExpandedExecution` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(PlanRequest::Synthetic)`, the command will become a `SpawnExecution` with the synthetic task.
    async fn get_plan_request(
        &mut self,
        program: &str,
        args: &[Str],
        path_env: Option<&Arc<OsStr>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<PlanRequest>>;
}

#[async_trait::async_trait(?Send)]
pub trait TaskGraphLoader {
    async fn load_task_graph(
        &mut self,
    ) -> Result<&vite_task_graph::IndexedTaskGraph, TaskGraphLoadError>;
}

#[derive(Debug, Serialize)]
pub struct ExecutionPlan {
    root_node: ExecutionItemKind,
}

impl ExecutionPlan {
    pub fn root_node(&self) -> &ExecutionItemKind {
        &self.root_node
    }

    pub async fn plan(
        plan_request: PlanRequest,
        workspace_path: &Arc<AbsolutePath>,
        cwd: &Arc<AbsolutePath>,
        envs: &HashMap<Arc<OsStr>, Arc<OsStr>>,
        plan_request_parser: &mut (dyn PlanRequestParser + '_),
        task_graph_loader: &mut (dyn TaskGraphLoader + '_),
    ) -> Result<Self, Error> {
        let workspace_node_modules_bin = workspace_path.join("node_modules").join(".bin");
        let mut envs = envs.clone();
        prepend_path_env(&mut envs, &workspace_node_modules_bin)
            .map_err(|join_paths_error| TaskPlanErrorKind::AddNodeModulesBinPathError {
                join_paths_error,
            })
            .with_empty_call_stack()?;
        let root_node = match plan_request {
            PlanRequest::Query(query_plan_request) => {
                let indexed_task_graph = task_graph_loader
                    .load_task_graph()
                    .await
                    .map_err(|load_error| TaskPlanErrorKind::TaskGraphLoadError(load_error))
                    .with_empty_call_stack()?;

                let context = PlanContext::new(
                    workspace_path,
                    Arc::clone(cwd),
                    envs.clone(),
                    plan_request_parser,
                    &indexed_task_graph,
                );
                let execution_graph = plan_query_request(query_plan_request, context).await?;
                ExecutionItemKind::Expanded(execution_graph)
            }
            PlanRequest::Synthetic(synthetic_plan_request) => {
                let execution = plan_synthetic_request(
                    workspace_path,
                    &Default::default(),
                    synthetic_plan_request,
                    None,
                    cwd,
                    &envs,
                )
                .with_empty_call_stack()?;
                ExecutionItemKind::Leaf(LeafExecutionKind::Spawn(execution))
            }
        };
        Ok(Self { root_node })
    }
}

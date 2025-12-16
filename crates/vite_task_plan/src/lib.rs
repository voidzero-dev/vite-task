mod context;
mod envs;
mod error;
mod execution_graph;
mod in_process;
mod leaf;
mod path_env;
mod plan;
pub mod plan_request;

use std::{collections::HashMap, ffi::OsStr, fmt::Debug, ops::Range, sync::Arc};

use context::PlanContext;
use envs::ResolvedEnvs;
use execution_graph::ExecutionGraph;
use in_process::InProcessExecution;
use plan_request::PlanRequest;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskNodeIndex, query::TaskQuery};

/*
/// Where an execution originates from
#[derive(Debug)]
pub enum ExecutionOrigin {
    /// the execution originates from the task graph (defined in `package.json` or `vite.config.*`)
    ///
    /// The precise location of this execution in the task graph can be inferred by
    /// `ExecutionGraphNode.task_index` and index of `ExecutionItem` in `ExecutionGraphNode.items`.
    TaskGraph,

    /// the command originates from an synthetic command, like `oxlint ...` synthesized from `vite lint`
    Synthetic
}
 */

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
    /*
        /// Where this resolved command originates from
        pub origin: ExecutionOrigin,
    */
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

    /// A task's command is splitted by `&&` and expanded into multiple execution items.
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

/// An execution item, from a splitted subcommand in a task's command (`item1 && item2 && ...`).
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
    /// This is called for every parsable command in the task graph in order to determine how to execute it.
    ///
    /// `vite_task_plan` doesn't have the knowledge of how cli args should be parsed. It relies on this callback.
    ///
    /// - If it returns `Err`, the planning will abort with the returned error.
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(ParsedArgs::TaskQuery)`, the command will be expanded as a `ExpandedExecution` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(ParsedArgs::Synthetic)`, the command will become a `SpawnExecution` with the synthetic task.
    fn parse_as_task_request(
        &self,
        program: &str,
        args: &[Str],
    ) -> anyhow::Result<Option<PlanRequest>>;
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

    pub async fn plan(context: PlanContext<'_>) {}
}

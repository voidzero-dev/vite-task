mod context;
mod envs;
mod error;
mod expand;
mod leaf;

use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    fmt::Debug,
    hash::Hash,
    ops::Range,
    sync::Arc,
};

use context::PlanContext;
use envs::ResolvedEnvs;
use futures_core::future::BoxFuture;
use futures_util::FutureExt;
use petgraph::graph::DiGraph;
use vite_path::AbsolutePath;
use vite_shell::TaskParsedCommand;
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNode, TaskNodeIndex, query::TaskQuery};

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

/// Resolved cache configuration for a leaf execution.
#[derive(Debug)]
pub struct ResolvedCacheConfig {
    /// Environment variables that are used for fingerprinting the cache.
    pub resolved_envs: ResolvedEnvs,
}

/// A resolved leaf execution.
/// Unlike tasks in `vite_task_graph`, this struct contains all information needed for execution,
/// like resolved environment variables, current working directory, and additional args from cli.
#[derive(Debug)]
pub struct LeafExecutionItem {
    /*
        /// Where this resolved command originates from
        pub origin: ExecutionOrigin,
    */
    /// Resolved cache configuration for this execution. `None` means caching is disabled.
    pub resolved_cache_config: Option<ResolvedCacheConfig>,

    /// Environment variables to set for the command, including both fingerprinted and pass-through envs.
    pub all_envs: Arc<HashMap<Str, Arc<str>>>,

    /// Current working directory
    pub cwd: Arc<AbsolutePath>,

    /// parsed program with args or shell script
    pub command_kind: LeafCommandKind,
}

pub enum LeafTaskResolutionError {}

impl LeafExecutionItem {
    pub fn resolve_from_task(
        task_node: &TaskNode,
        context: PlanContext,
    ) -> Result<Self, LeafTaskResolutionError> {
        todo!()
    }
}

/// The kind of a leaf execution
#[derive(Debug)]
pub enum LeafCommandKind {
    /// A program with args to be executed directly
    Program { program: Str, args: Arc<[Str]> },
    /// A script to be executed by os shell
    ShellScript(Str),
}

/// A node in the execution graph, coresponding to a task.
#[derive(Debug)]
pub struct ExecutionGraphNode {
    /// The task index in the task graph
    pub task_index: TaskNodeIndex,

    /// A task's command is splitted by `&&` and expanded into multiple execution items.
    ///
    /// It contains a single item if the command has no `&&`
    pub items: Vec<ExecutionItem>,
}

/// An execution item, either expanded from a known vite subcommand, or a leaf execution.
#[derive(Debug)]
pub struct ExecutionItem {
    /// The range of the task command that this execution item is resolved from.
    ///
    /// This field is for displaying purpose only.
    /// The actual execution info (if this is leaf) is in `LeafExecutionItem.command_kind`.
    pub command_span: Range<usize>,

    /// The kind of this execution item
    pub kind: ExecutionItemKind,
}

/// An execution item, from a splitted subcommand in a task's command (`item1 && item2 && ...`).
#[derive(Debug)]
pub enum ExecutionItemKind {
    /// Expanded from a known vite subcommand, like `vite run ...` or `vite lint`.
    Expanded(DiGraph<ExecutionGraphNode, ()>),
    /// A normal leaf execution, like `tsc --noEmit`.
    Leaf(LeafExecutionItem),
}

/// Callbackes needed during planning.
/// See each method for details.
pub trait PlanCallbacks: Debug {
    fn load_task_graph<'me>(
        &'me self,
    ) -> BoxFuture<'me, Result<&'me IndexedTaskGraph, vite_task_graph::TaskGraphLoadError>>;

    /// This is called for every parsable command in the task graph in order to determine how to execute it.
    ///
    /// `vite_task_plan` doesn't have the knowledge of how cli args should be parsed. It relies on this callback.
    ///
    /// - If it returns `Err`, the planning will abort with the returned error.
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(ParsedArgs::TaskQuery)`, the command will be expanded as a `ExpandedExecution` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(ParsedArgs::Synthetic)`, the command will become a `LeafExecution` with the synthetic task.
    fn parse_args(&self, program: &str, args: &[Str]) -> anyhow::Result<Option<Subcommand>>;
}

/// The command arguments indicating to run tasks queried from the task graph.
/// For example: `vite run -r build -- arg1 arg2`
#[derive(Debug)]
pub struct QueryTasksSubcommand {
    /// The query to run against the task graph. For example: `-r build`
    pub query: TaskQuery,

    /// Other options affecting the planning process, not the task graph querying itself.
    ///
    /// For example: `-- arg1 arg2`
    pub plan_options: PlanOptions,
}

/// The parsed command arguments.
#[derive(Debug)]
pub enum Subcommand {
    /// The args indicate to run tasks queried from the task graph, like `vite run -r build -- arg1 arg2`.
    QueryTasks(QueryTasksSubcommand),
    /// The args indicate to run a synthetic task, like `vite lint`.
    Synthetic { name: Str, extra_args: Arc<[Str]> },
}

#[derive(Debug)]
pub struct PlanOptions {
    pub extra_args: Arc<[Str]>,
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

    pub async fn plan(&self, args: Subcommand, context: PlanContext<'_>) {}
}

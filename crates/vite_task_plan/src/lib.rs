mod envs;
mod expand;
mod leaf;

use std::{collections::HashMap, fmt::Debug, sync::Arc};

use envs::ResolvedEnvs;
use futures_core::future::BoxFuture;
use futures_util::FutureExt;
use petgraph::graph::DiGraph;
use vite_path::AbsolutePath;
use vite_shell::ParsedScript;
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNode, TaskNodeIndex, query::TaskQuery};

/// Where an execution originates from
#[derive(Debug)]
pub enum ExecutionOrigin {
    /// the execution originates from the task graph (defined in `package.json` or `vite.config.*`)
    TaskGraph {
        /// the task index in the task graph
        task_index: TaskNodeIndex,
        /// the index of the subcommand in parsed script `subcommand0 && subcommand1 ...`.
        ///
        /// 0 if the script is not parsable.
        subcommand: usize,
    },

    /// the command originates from an synthetic command, like `oxlint ...` synthesized from `vite lint`
    Synthetic {
        /// the name of the synthetic command.
        /// This is going to be part of associated task name in cache, so that a second `vite lint` can
        /// report cache miss compared to the first one.
        name: Str,
    },
}

/// A resolved leaf execution.
/// Unlike tasks in `vite_task_graph`, this struct contains all information needed for execution,
/// like resolved environment variables, current working directory, and additional args from cli.
#[derive(Debug)]
pub struct LeafExecutionItem {
    /// Where this resolved command originates from
    pub origin: ExecutionOrigin,

    /// Environment variables to set for the command
    pub resolved_envs: ResolvedEnvs,

    /// Current working directory
    pub cwd: Arc<AbsolutePath>,

    /// parsed program with args or shell script
    pub kind: LeafExecutionKind,
}

pub enum LeafTaskResolutionError {}

impl LeafExecutionItem {
    pub fn resolve_from_task(
        task_node: &TaskNode,
        context: PlanContext<'_>,
    ) -> Result<Self, LeafTaskResolutionError> {
        todo!()
    }
}

/// The kind of a leaf execution
#[derive(Debug)]
pub enum LeafExecutionKind {
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

#[derive(Debug)]
pub enum ExecutionItemScript {
    Parsed(ParsedScript),
    ShellScript(Str),
}

/// An execution item, either expanded from a known vite subcommand, or a leaf execution.
#[derive(Debug)]
pub struct ExecutionItem {
    /// The script that this execution item is resolved from.
    ///
    /// This field is for displaying purpose only. The actual execution info is in `kind`.
    pub script: ExecutionItemScript,

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
    fn load_task_graph<'s>(
        &'s mut self,
    ) -> BoxFuture<'s, Result<&'s IndexedTaskGraph, vite_task_graph::TaskGraphLoadError>>;

    /// This is called for every parsable command in order to determine how to expand it.
    ///
    /// `vite_task_plan` doesn't have the knowledge of how cli args should be parsed. It relies on this callback
    ///
    /// - If it returns `Err`, the planning will abort with the returned error.
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(ParsedArgs::QueryTaskGraph)`, the command will be expanded as a `ExpandedExecution` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(ParsedArgs::Synthetic)`, the command will expanded as a `ExpandedExecution` with a task graph containing the synthetic task.
    fn parse_into_expansion_args(
        &mut self,
        program: &str,
        args: &[Str],
    ) -> anyhow::Result<Option<ExpansionArgs>>;
}

/// The context for planning an execution from a task.
#[derive(Debug)]
pub struct PlanContext<'a> {
    pub cwd: Arc<AbsolutePath>,
    pub envs: HashMap<Str, Arc<str>>,
    pub callbacks: &'a mut dyn PlanCallbacks,
}

/// The parsed cli arguments for expansion.
#[derive(Debug)]
pub enum ExpansionArgs {
    QueryTaskGraph { query: TaskQuery, plan_options: PlanOptions },
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

    pub async fn plan(&self, args: ExpansionArgs, context: PlanContext<'_>) {}
}

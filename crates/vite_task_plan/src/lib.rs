mod envs;

use std::{collections::HashMap, fmt::Debug, sync::Arc};

use envs::ResolvedEnvs;
use futures_core::future::BoxFuture;
use futures_util::FutureExt;
use petgraph::graph::DiGraph;
use vite_path::AbsolutePath;
use vite_shell::ParsedScript;
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskNodeIndex, query::TaskQuery};

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
pub struct LeafExecution {
    /// Where this resolved command originates from
    pub origin: ExecutionOrigin,

    /// Environment variables to set for the command
    pub resolved_envs: ResolvedEnvs,

    /// Current working directory
    pub cwd: Arc<AbsolutePath>,

    /// parsed program with args or shell script
    pub kind: LeafExecutionKind,
}

/// The kind of a leaf execution
#[derive(Debug)]
pub enum LeafExecutionKind {
    /// A program with args to be executed directly
    Program { program: Str, args: Arc<[Str]> },
    /// A script to be executed by os shell
    ShellScript(Str),
}

/// A group execution containing a graph of sub-executions, expanded from a parsed script, like `vite run ...` or `vite lint`.
#[derive(Debug)]
pub struct GroupExecution {
    /// The script that this group is expanded from. For displaying purpose.
    expanded_from: ParsedScript,

    /// The expanded execution nodes in this group
    execution_graph: DiGraph<ExecutionNode, ()>,
}

/// An execution node, either a group or a resolved command
#[derive(Debug)]
pub enum ExecutionNode {
    /// A group of execution nodes, expanded from a parsed script, like `vite run ...` or `vite lint`.
    Group(GroupExecution),
    /// A leaf execution ready, like `tsc --noEmit`.
    Leaf(LeafExecution),
}

pub trait PlanCallbacks: Debug {
    fn load_task_graph<'s>(
        &'s mut self,
    ) -> BoxFuture<'s, Result<&'s IndexedTaskGraph, vite_task_graph::TaskGraphLoadError>>;
    fn parse_args(&mut self, program: &str, args: &[Str]) -> ParsedArgs;
}

/// The context for planning an execution from a task.
#[derive(Debug)]
pub struct PlanContext<'a> {
    pub cwd: Arc<AbsolutePath>,
    pub envs: HashMap<Str, Arc<str>>,
    pub callbacks: &'a dyn PlanCallbacks,
}

#[derive(Debug)]
pub enum ParsedArgs {
    QueryTaskGraph { query: TaskQuery, plan_options: PlanOptions },
    Synthetic { name: Str, extra_args: Arc<[Str]> },
}

#[derive(Debug)]
pub struct PlanOptions {
    pub extra_args: Arc<[Str]>,
}

#[derive(Debug)]
pub struct ExecutionPlan {
    /// The plan starts from a root group, expanded from the cli args
    root_group: GroupExecution,
}

pub struct Args {
    pub query: TaskQuery,
}

impl ExecutionPlan {
    pub fn root_group(&self) -> &GroupExecution {
        &self.root_group
    }

    pub async fn plan(&self, args: ParsedArgs, context: PlanContext<'_>) {}
}

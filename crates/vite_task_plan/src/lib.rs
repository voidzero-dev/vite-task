mod envs;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use envs::ResolvedEnvs;
use vite_path::AbsolutePath;
use vite_shell::ParsedScript;
use vite_str::Str;
use vite_task_graph::TaskNodeIndex;

/// Where a resolved command originates from
#[derive(Debug)]
pub enum ResolvedCommandOrigin {
    /// the command originates from a task
    Task {
        /// the task index in the task graph
        task_index: TaskNodeIndex,
        /// the index of the subcommand in parsed script `subcommand0 && subcommand1 ...`.
        ///
        /// 0 if the script is not parsable.
        subcommand: usize,
    },

    /// the command originates from an synthetic command, like `oxlint ...` synthesized from `vite lint`
    Synthetic {
        /// the name of the synthetic command
        name: Str,
    },
}

/// A resolved environment variable value for a command
#[derive(Debug)]
struct ResolvedEnvValue {
    /// The value of the environment variable
    pub value: Str,
    /// Whether the environment variable should be passed through without being fingerprinted
    pub is_pass_through: bool,
}

/// A resolved command ready for execution
#[derive(Debug)]
pub struct ResolvedCommand {
    /// Where this resolved command originates from
    origin: ResolvedCommandOrigin,

    /// Environment variables to set for the command
    resolved_envs: ResolvedEnvs,

    /// Current working directory
    cwd: Arc<AbsolutePath>,

    /// parsed program with args or shell script
    kind: ResolvedCommandKind,
}

#[derive(Debug)]
pub enum ResolvedCommandKind {
    Parsed { program: Str, args: Arc<[Str]> },
    ShellScript(Str),
}

pub struct ExecutionNode {}

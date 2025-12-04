use std::collections::{BTreeMap, HashMap};

use vite_str::Str;

/// The command to run for a task
#[derive(Debug, PartialEq, Eq)]
pub enum TaskCommand {
    /// The command is unparsed shell script because of unsupported shell syntaxes
    ShellScript(Str),
    /// The command is parsed into program and args
    Parsed(TaskParsedCommand),
}

/// A parsed command: "FOO=BAR program arg1 arg2"
#[derive(Debug, PartialEq, Eq)]
pub struct TaskParsedCommand {
    pub envs: HashMap<Str, Str>,
    pub program: Str,
    pub args: Box<[Str]>,
}

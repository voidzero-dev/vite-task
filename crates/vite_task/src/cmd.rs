use std::{collections::BTreeMap, fmt::Display};

use bincode::{Decode, Encode};
use diff::Diff;
use serde::{Deserialize, Serialize};
use vite_str::Str;

/// Parsed command structure for built-in commands
/// "FOO=BAR program arg1 arg2"
#[derive(Encode, Decode, Serialize, Deserialize, Debug, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
pub struct TaskParsedCommand {
    pub envs: BTreeMap<Str, Str>,
    pub program: Str,
    pub args: Vec<Str>,
}

impl Display for TaskParsedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // BTreeMap ensures stable iteration order
        for (name, value) in &self.envs {
            Display::fmt(
                &format_args!("{}={} ", name, shell_escape::escape(value.as_str().into())),
                f,
            )?;
        }
        Display::fmt(&shell_escape::escape(self.program.as_str().into()), f)?;
        for arg in &self.args {
            Display::fmt(" ", f)?;
            Display::fmt(&shell_escape::escape(arg.as_str().into()), f)?;
        }

        Ok(())
    }
}

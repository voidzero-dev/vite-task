//! The document `vp run --json` prints: every package and task in the workspace, and
//! how the tasks depend on each other.
//!
//! The doc comments here are the descriptions in `schemas/task-graph.json`, the JSON
//! Schema that `$schema` points to, which a test generates from these types.

use std::{collections::BTreeMap, sync::Arc};

use serde::Serialize;
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::session::json::Version;

/// Every package and task in the workspace, and how the tasks depend on each other, as
/// printed by `vp run --json`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct TaskGraph {
    /// `file://` URL of the JSON Schema that describes this document.
    #[serde(rename = "$schema")]
    pub schema: Str,

    /// Format version. It changes only for a breaking change: a rename, a removal, or a
    /// changed meaning. New fields and values don't change it; ignore fields you don't know.
    pub version: Version,

    /// Absolute path of the workspace root. Other paths are relative to it, with `/` on
    /// every platform.
    #[cfg_attr(test, schemars(with = "Str"))]
    pub workspace_root: Arc<AbsolutePath>,

    /// Every package in the workspace, keyed by path (`"."` for the root), even ones
    /// without tasks.
    pub packages: BTreeMap<Str, Package>,

    /// Every task, keyed by task ID: `<package path>#<task name>`, e.g. `packages/ui#build`,
    /// or just the task name for a task of the root package. In each part, `%` is written
    /// `%25` and `#` is written `%23`. Compare IDs; don't decode them.
    pub tasks: BTreeMap<Str, Task>,
}

/// A package in the workspace.
#[derive(Debug, Default, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Package {
    /// The name in `package.json`. Absent when the package has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Str"))]
    pub name: Option<Str>,

    /// The other workspace packages in `dependencies` in `package.json`, by path. Packages
    /// from outside the workspace aren't listed. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<Str>,

    /// The other workspace packages in `devDependencies` in `package.json`, by path.
    /// Packages from outside the workspace aren't listed. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dev_dependencies: Vec<Str>,

    /// The other workspace packages in `peerDependencies` in `package.json`, by path.
    /// Packages from outside the workspace aren't listed. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub peer_dependencies: Vec<Str>,
}

/// A task.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// The path of the task's package, which is also its key in `packages`.
    pub package: Str,

    /// The task name, from `vite.config.*` or a `package.json` script.
    pub name: Str,

    /// The directory the command runs in, when the task's `cwd` setting changes it. It is
    /// absolute when it's outside the workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Str"))]
    pub cwd: Option<Str>,

    /// The command as the task defines it.
    pub command: Command,

    /// The tasks in the task's own `dependsOn`, by ID. An entry such as
    /// `{ task: 'build', from: 'dependencies' }` is replaced by the tasks it selects.
    /// Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Str>,
}

/// A string, or an array of strings that run one after another. A single command is
/// always a string, even when the task defines it as a one-element array.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(test, schemars(inline))]
pub enum Command {
    Single(Str),
    Sequence(Vec<Str>),
}

impl Command {
    /// A string for one command, an array otherwise.
    #[must_use]
    pub fn new(commands: &[Str]) -> Self {
        match commands {
            [command] => Self::Single(command.clone()),
            commands => Self::Sequence(commands.to_vec()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        let json = |commands: &[&str]| {
            let commands: Vec<Str> = commands.iter().copied().map(Str::from).collect();
            serde_json::to_value(Command::new(&commands)).unwrap()
        };
        assert_eq!(json(&["tsdown"]), serde_json::json!("tsdown"));
        assert_eq!(json(&["tsc", "tsdown"]), serde_json::json!(["tsc", "tsdown"]));
    }
}

//! The run plan `vp run --dry-run --json` prints: the tasks a run would run, in what order,
//! and whether each step would hit the cache.
//!
//! The doc comments here are the descriptions in `schemas/run-plan.json`, the JSON Schema that
//! `$schema` points to, which a test generates from these types.

use std::{collections::BTreeMap, sync::Arc};

use serde::Serialize;
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::session::json::Version;

/// The tasks a run would run, in what order, and whether each step would hit the cache, as
/// printed by `vp run --dry-run --json`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunPlan {
    /// `file://` URL of the JSON Schema that describes this document.
    #[serde(rename = "$schema")]
    pub schema: Str,

    /// Format version. It changes only for a breaking change: a rename, a removal, or a
    /// changed meaning. New fields and values don't change it; ignore fields you don't know.
    pub version: Version<1>,

    /// Absolute path of the workspace root. Other paths are relative to it, with `/` on
    /// every platform, except paths outside the workspace, which are absolute.
    #[cfg_attr(test, schemars(with = "Str"))]
    pub workspace_root: Arc<AbsolutePath>,

    /// The tasks this run would run, keyed by task ID, the same IDs as in `vp run --json`.
    pub plan: BTreeMap<Str, PlanTask>,
}

/// A task the run would run.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlanTask {
    /// The path of the task's package, such as `packages/ui`, or `.` for the workspace root.
    pub package: Str,

    /// The task name, such as `build`.
    pub name: Str,

    /// IDs of the tasks in the same `plan` that must finish before this one starts. Absent
    /// when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Str>,

    /// What the task runs, in order. Each part of the command between `&&`, each entry of a
    /// command array, and each `pre`/`post` script is one step. Each step is cached
    /// separately.
    pub steps: Vec<Step>,
}

/// A step of a task: a command, or a nested `vp run`.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum Step {
    Command(CommandStep),
    Nested(NestedStep),
}

/// A nested `vp run`, which runs a plan of its own.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct NestedStep {
    /// The `vp run …` command this step stands for, as written, with any arguments given
    /// after the task name: `vp run -r test`.
    pub expanded_from: Str,

    /// The plan of that nested `vp run`, in the same shape. Its tasks' `dependsOn` refer only
    /// to tasks in it, so the same ID can appear in more than one plan.
    pub plan: BTreeMap<Str, PlanTask>,
}

/// A command the task runs.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CommandStep {
    /// The executable that would be started: a path relative to the workspace root if it is
    /// inside it, such as `apps/web/node_modules/.bin/vite`,
    /// otherwise an absolute path. A command with shell syntax, such as a pipe, runs through
    /// the shell: `/bin/sh` with `-c` and the command, or `cmd.exe` with `/d /s /c` on
    /// Windows. For a built-in such as `echo`, which runs without a new process, this is
    /// just its name.
    pub program: Str,

    /// The arguments, including any given after the task name. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<Str>,

    /// The directory it runs in, when the task's `cwd` setting or an earlier `cd` in the
    /// command changes it. Absent when it runs in the package directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Str"))]
    pub cwd: Option<Str>,

    /// The `pre` or `post` script the step comes from, such as `prebuild`. Absent for the
    /// task's own command.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Str"))]
    pub hook: Option<Str>,

    /// Whether the cache would be used. Absent for a built-in such as `echo`.
    ///
    /// The check looks at the files as they are now.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Cache"))]
    pub cache: Option<Cache>,
}

/// Whether a step would use the cache, and why it would run.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Cache {
    /// Matches `baseline`, so it would replay `baseline` instead of running.
    Hit(Hit),

    /// Differs from `baseline`, so it would run. `changes` lists the differences.
    Changed(Changed),

    /// Would run: there is no cached run to compare with, as on a first run or after
    /// `vp cache clean`. Arguments given after the task name count, so a new combination of
    /// arguments is also `noBaseline`.
    NoBaseline,

    /// Would run: the cache couldn't be read. For example, the remote cache failed and
    /// nothing is cached locally.
    Error {
        /// What went wrong.
        message: Str,
    },

    /// Would run without caching, because caching is off for this step.
    Disabled,
}

/// A step that matches its cached run.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// The cached run the step matches.
    pub baseline: Baseline,
}

/// A step that differs from its cached run.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    /// The cached run the step is compared with, which has the same command, config and env.
    /// Absent when the command, config or env changed: then `changes` lists only those.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, schemars(with = "Baseline"))]
    pub baseline: Option<Baseline>,

    /// What differs from the cached run, sorted by kind in the order listed here, then by name
    /// or path. Undoing all of them would make the step a hit, unless `changesPartial` is set.
    pub changes: Vec<Change>,

    /// Other files may have changed too: without `-v`, the file check stops at the first
    /// changed file it finds, and after a command, config or env change, it doesn't run.
    /// Absent when every change is listed.
    #[serde(skip_serializing_if = "is_false")]
    #[cfg_attr(test, schemars(with = "AlwaysTrue"))]
    pub changes_partial: bool,
}

/// The cached run a step is compared with.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    /// How long the cached run took, in milliseconds. On a hit, the time a replay saves.
    pub duration_ms: u64,
}

/// One difference between a step and its cached run.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Change {
    /// The program changed. Each is the program as the cache records it: a path inside the
    /// workspace, or just the file name outside it, so `node` from another install is not a
    /// change.
    Program { from: Str, to: Str },

    /// The arguments changed.
    Args { from: Vec<Str>, to: Vec<Str> },

    /// The working directory changed. Each is relative to the workspace root.
    Cwd { from: Str, to: Str },

    /// The setting changed. A changed `input` is one change: the files it now matches aren't
    /// listed.
    Config { setting: Setting },

    /// An env var was added, removed, or changed value.
    Env { name: Str, change: HowChanged },

    /// A file was added, removed, or changed. `added` also covers a new file in a directory
    /// the task listed.
    File { path: Str, change: HowChanged },
}

/// A task setting that affects caching.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum Setting {
    Input,
    Output,
    UntrackedEnv,
}

/// How an env var or a file changed.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum HowChanged {
    Added,
    Removed,
    Changed,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` passes a reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// `true`, the only value a flag that is absent when unset has.
#[cfg(test)]
struct AlwaysTrue;

#[cfg(test)]
impl schemars::JsonSchema for AlwaysTrue {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("AlwaysTrue")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "boolean", "const": true })
    }
}

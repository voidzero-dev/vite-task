mod name;
mod task_command;
mod task_graph_builder;
mod workspace;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    future::Future,
    sync::Arc,
};

use bincode::{Decode, Encode};
use compact_str::ToCompactString;
use diff::Diff;
use serde::{Deserialize, Serialize};
pub use task_command::*;
pub use task_graph_builder::*;
use vite_path::{self, RelativePath, RelativePathBuf};
use vite_shell::TaskParsedCommand;
use vite_str::Str;
pub use workspace::*;

use crate::{
    Error,
    collections::{HashMap, HashSet},
    config::name::TaskName,
    execute::TaskEnvs,
    types::ResolveCommandResult,
};

#[derive(Encode, Decode, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Diff)]
#[diff(attr(#[derive(Debug)]))]
#[serde(rename_all = "camelCase")]
pub struct TaskConfig {
    pub(crate) command: TaskCommand,
    #[serde(default)]
    pub(crate) cwd: RelativePathBuf,
    pub(crate) cacheable: bool,

    #[serde(default)]
    pub(crate) inputs: HashSet<Str>,

    #[serde(default)]
    pub(crate) envs: HashSet<Str>,

    #[serde(default)]
    pub(crate) pass_through_envs: HashSet<Str>,

    #[serde(default)]
    pub(crate) fingerprint_ignores: Option<Vec<Str>>,
}

impl TaskConfig {
    pub fn set_fingerprint_ignores(&mut self, fingerprint_ignores: Option<Vec<Str>>) {
        self.fingerprint_ignores = fingerprint_ignores;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskConfigWithDeps {
    #[serde(flatten)]
    pub(crate) config: TaskConfig,
    #[serde(default)]
    pub(crate) depends_on: Vec<Str>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ViteTaskJson {
    pub(crate) tasks: HashMap<Str, TaskConfigWithDeps>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct DisplayOptions {
    /// Whether to hide the command ("~> echo hello") before the execution.
    pub hide_command: bool,

    /// Whether to hide this task in the summary after all executions.
    pub hide_summary: bool,

    /// If true, the task will not be replayed from the cache.
    /// This is useful for tasks that should not be replayed, like auto run install command.
    /// TODO: this is a temporary solution, we should find a better way to handle this.
    pub ignore_replay: bool,
}

/// A resolved task, ready to hit the cache or be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTask {
    pub name: TaskName,
    pub args: Arc<[Str]>,
    pub resolved_config: ResolvedTaskConfig,
    pub resolved_command: ResolvedTaskCommand,
    pub display_options: DisplayOptions,
}

impl ResolvedTask {
    pub fn id(&self) -> TaskId {
        TaskId {
            subcommand_index: self.name.subcommand_index,
            task_group_id: TaskGroupId {
                task_group_name: self.name.task_group_name.clone(),
                config_path: self.resolved_config.config_dir.clone(),
                is_builtin: self.is_builtin(),
            },
        }
    }

    pub const fn is_builtin(&self) -> bool {
        self.name.package_name.is_none()
    }

    pub fn matches(&self, task_request: &str, current_package_path: Option<&RelativePath>) -> bool {
        if self.name.subcommand_index.is_some() {
            // never match non-last subcommand
            return false;
        }

        let Some(package_name) = &self.name.package_name else {
            // never match built-in task
            return false;
        };

        // match tasks in current package if the task_request doesn't contain '#'
        if !task_request.contains('#') {
            return current_package_path == Some(&self.resolved_config.config_dir)
                && self.name.task_group_name == task_request;
        }

        task_request.get(..package_name.len()) == Some(package_name)
            && task_request.get(package_name.len()..=package_name.len()) == Some("#")
            && task_request.get(package_name.len() + 1..) == Some(&self.name.task_group_name)
    }

    /// For displaying in the UI.
    /// Not necessarily a unique identifier as the package name can be duplicated.
    pub fn display_name(&self) -> Str {
        self.name.to_compact_string().into()
    }

    #[tracing::instrument(skip(workspace, resolve_command, args))]
    /// Resolve a built-in task, like `vite lint`, `vite build`
    pub async fn resolve_from_builtin<
        Resolved: Future<Output = Result<ResolveCommandResult, Error>>,
        ResolveFn: Fn() -> Resolved,
    >(
        workspace: &Workspace,
        resolve_command: ResolveFn,
        task_name: &str,
        args: impl Iterator<Item = impl AsRef<str>> + Clone,
    ) -> Result<Self, Error> {
        let ResolveCommandResult { bin_path, envs } = resolve_command().await?;
        Self::resolve_from_builtin_with_command_result(
            workspace,
            task_name,
            args,
            ResolveCommandResult { bin_path, envs },
            false,
            None,
        )
    }

    pub fn resolve_from_builtin_with_command_result(
        workspace: &Workspace,
        task_name: &str,
        args: impl Iterator<Item = impl AsRef<str>> + Clone,
        command_result: ResolveCommandResult,
        ignore_replay: bool,
        fingerprint_ignores: Option<Vec<Str>>,
    ) -> Result<Self, Error> {
        let ResolveCommandResult { bin_path, envs } = command_result;
        let builtin_task = TaskCommand::Parsed(TaskParsedCommand {
            args: args.clone().map(|arg| arg.as_ref().into()).collect(),
            envs: envs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            program: bin_path.into(),
        });
        let mut task_config: TaskConfig = builtin_task.clone().into();
        task_config.set_fingerprint_ignores(fingerprint_ignores.clone());
        let pass_through_envs = task_config.pass_through_envs.iter().cloned().collect();
        let cwd = &workspace.cwd;
        let resolved_task_config =
            ResolvedTaskConfig { config_dir: cwd.clone(), config: task_config };
        let resolved_envs =
            TaskEnvs::resolve(std::env::vars_os(), &workspace.root_dir, &resolved_task_config)?;
        let resolved_command = ResolvedTaskCommand {
            fingerprint: CommandFingerprint {
                cwd: cwd.clone(),
                command: builtin_task,
                envs_without_pass_through: resolved_envs
                    .envs_without_pass_through
                    .into_iter()
                    .collect(),
                pass_through_envs,
                fingerprint_ignores,
            },
            all_envs: resolved_envs.all_envs,
        };
        Ok(Self {
            name: TaskName {
                package_name: None,
                task_group_name: task_name.into(),
                subcommand_index: None,
            },
            args: args.map(|arg| arg.as_ref().into()).collect(),
            resolved_config: resolved_task_config,
            resolved_command,
            display_options: DisplayOptions {
                // built-in tasks don't show the actual command.
                // For example, `vite lint`'s actual command is the path to the bundled oxlint,
                // We don't want to show that to the user.
                //
                // When built-in command like `vite lint` is run as the script of a user-defined task, the script itself
                // will be displayed as the command in the inner runner.
                hide_command: true,
                hide_summary: false,
                ignore_replay,
            },
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResolvedTaskCommand {
    pub fingerprint: CommandFingerprint,
    pub all_envs: HashMap<Str, Arc<OsStr>>,
}

impl std::fmt::Debug for ResolvedTaskCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if std::env::var("VITE_DEBUG_VERBOSE").map(|v| v != "0" && v != "false").unwrap_or(false) {
            write!(
                f,
                "ResolvedTaskCommand {{ fingerprint: {:?}, all_envs: {:?} }}",
                self.fingerprint, self.all_envs
            )
        } else {
            write!(f, "ResolvedTaskCommand {{ fingerprint: {:?} }}", self.fingerprint)
        }
    }
}

/// Fingerprint for command execution that affects caching.
///
/// # Environment Variable Impact on Cache
///
/// The `envs_without_pass_through` field is crucial for cache correctness:
/// - Only includes envs explicitly declared in the task's `envs` array
/// - Does NOT include pass-through envs (PATH, CI, etc.)
/// - These envs become part of the cache key
///
/// When a task runs:
/// 1. All envs (including pass-through) are available to the process
/// 2. Only declared envs affect the cache key
/// 3. If a declared env changes value, cache will miss
/// 4. If a pass-through env changes, cache will still hit
///
/// For built-in tasks (lint, build, etc):
/// - The resolver provides envs which become part of the fingerprint
/// - If resolver provides different envs between runs, cache breaks
/// - Each built-in task type must have unique task name to avoid cache collision
///
/// # Fingerprint Ignores Impact on Cache
///
/// The `fingerprint_ignores` field controls which files are tracked in `PostRunFingerprint`:
/// - Changes to this config must invalidate the cache
/// - Vec maintains insertion order (pattern order matters for last-match-wins semantics)
/// - Even though ignore patterns only affect `PostRunFingerprint`, the config itself is part of the cache key
#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
pub struct CommandFingerprint {
    pub cwd: RelativePathBuf,
    pub command: TaskCommand,
    /// Environment variables that affect caching (excludes pass-through envs)
    pub envs_without_pass_through: BTreeMap<Str, Str>, // using BTreeMap to have a stable order in cache db

    /// even though value changes to `pass_through_envs` shouldn't invalidate the cache,
    /// The names should still be fingerprinted so that the cache can be invalidated if the `pass_through_envs` config changes
    pub pass_through_envs: BTreeSet<Str>, // using BTreeSet to have a stable order in cache db

    /// Glob patterns for fingerprint filtering. Order matters (last match wins).
    /// Changes to this config invalidate the cache to ensure correct fingerprint tracking.
    pub fingerprint_ignores: Option<Vec<Str>>,
}

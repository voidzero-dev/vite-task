use std::fmt::Display;

use bincode::{Decode, Encode};
use diff::Diff;
use serde::{Deserialize, Serialize};
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;

use super::{CommandFingerprint, ResolvedTaskCommand, TaskConfig};
use crate::{Error, cmd::TaskParsedCommand, execute::TaskEnvs};

#[derive(Encode, Decode, Serialize, Deserialize, Debug, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
#[serde(untagged)]
pub enum TaskCommand {
    ShellScript(Str),
    Parsed(TaskParsedCommand),
}

impl Display for TaskCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShellScript(script) => Display::fmt(script, f),
            Self::Parsed(parsed) => Display::fmt(parsed, f),
        }
    }
}

impl From<TaskCommand> for TaskConfig {
    fn from(command: TaskCommand) -> Self {
        Self {
            command,
            cwd: RelativePathBuf::empty(),
            cacheable: true,
            inputs: Default::default(),
            envs: Default::default(),
            pass_through_envs: Default::default(),
            fingerprint_ignores: Default::default(),
        }
    }
}

impl TaskCommand {
    pub fn need_skip_cache(&self) -> bool {
        match self {
            Self::Parsed(parsed) => {
                parsed.program == "vite"
                    || (parsed.program.ends_with("vite.js")
                        && parsed.args.first() == Some(&("dev".into())))
            }
            Self::ShellScript(script) => {
                let cmd = script.trim();
                cmd.starts_with("vite dev") || cmd.starts_with("vite.js dev")
            }
        }
    }

    // Whether the command starts a inner runner.
    pub fn has_inner_runner(&self) -> bool {
        match self {
            Self::Parsed(parsed) => {
                if parsed.program != "vite" {
                    return false;
                }
                let Some(subcommand) = parsed.args.first() else {
                    return false;
                };
                matches!(subcommand.as_str(), "run" | "lint" | "fmt" | "build" | "test" | "lib")
            }
            Self::ShellScript(script) => {
                let cmd = script.trim();
                if !cmd.starts_with("vite ") {
                    return false;
                }
                let rest = &cmd[5..]; // Skip "vite "
                let subcommand = rest.split_whitespace().next().unwrap_or("");
                matches!(subcommand, "run" | "lint" | "fmt" | "build" | "test" | "lib")
            }
        }
    }
}

#[derive(Encode, Decode, Debug, Serialize, Deserialize, PartialEq, Eq, Diff, Clone)]
#[diff(attr(#[derive(Debug)]))]
pub struct ResolvedTaskConfig {
    pub config_dir: RelativePathBuf,
    pub config: TaskConfig,
}

impl ResolvedTaskConfig {
    pub(crate) fn resolve_command(
        &self,
        base_dir: &AbsolutePath,
        task_args: &[Str],
    ) -> Result<ResolvedTaskCommand, Error> {
        let cwd = self.config_dir.join(&self.config.cwd);
        let command = if task_args.is_empty() {
            self.config.command.clone()
        } else {
            match &self.config.command {
                TaskCommand::ShellScript(script) => {
                    let command_script =
                        std::iter::once(script.clone())
                            .chain(task_args.iter().map(|arg| {
                                shell_escape::escape(arg.as_str().into()).as_ref().into()
                            }))
                            .collect::<Vec<_>>()
                            .join(" ")
                            .into();
                    TaskCommand::ShellScript(command_script)
                }
                TaskCommand::Parsed(parsed) => {
                    let mut parsed = parsed.clone();
                    parsed.args.extend_from_slice(task_args);
                    TaskCommand::Parsed(parsed)
                }
            }
        };
        let task_envs = TaskEnvs::resolve(base_dir, self)?;
        Ok(ResolvedTaskCommand {
            fingerprint: CommandFingerprint {
                cwd,
                command,
                envs_without_pass_through: task_envs
                    .envs_without_pass_through
                    .into_iter()
                    .collect(),
                pass_through_envs: self.config.pass_through_envs.iter().cloned().collect(),
                fingerprint_ignores: self.config.fingerprint_ignores.clone(),
            },
            all_envs: task_envs.all_envs,
        })
    }
}

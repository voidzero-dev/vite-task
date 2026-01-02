use std::{
    env::{self, join_paths},
    ffi::OsStr,
    iter,
    path::PathBuf,
    sync::Arc,
};

use clap::Subcommand;
use vite_path::{AbsolutePath, current_dir};
use vite_str::Str;
use vite_task::{CLIArgs, Session, SessionCallbacks, plan_request::SyntheticPlanRequest};

/// Theses are the custom subcommands that synthesize tasks for vite-task
#[derive(Debug, Subcommand)]
pub enum CustomTaskSubcommand {
    /// oxlint
    Lint { args: Vec<Str> },
}

// These are the subcommands that is not handled by vite-task
#[derive(Debug, Subcommand)]
pub enum NonTaskSubcommand {
    Version,
}

#[derive(Debug, Default)]
pub struct TaskSynthesizer(());

fn find_executable_in_node_modules_bin(
    cwd: &AbsolutePath,
    executable: &str,
) -> anyhow::Result<Arc<OsStr>> {
    let mut paths: Vec<PathBuf> = vec![];
    let mut current_cwd_parent = cwd;
    loop {
        let node_modules_bin = current_cwd_parent.join("node_modules").join(".bin");
        paths.push(node_modules_bin.as_path().to_path_buf());
        if let Some(parent) = current_cwd_parent.parent() {
            current_cwd_parent = parent;
        } else {
            break;
        }
    }
    let executable_path = which::which_in(executable, Some(join_paths(paths)?), cwd)?;
    Ok(executable_path.into_os_string().into())
}

#[async_trait::async_trait(?Send)]
impl vite_task::TaskSynthesizer<CustomTaskSubcommand> for TaskSynthesizer {
    fn should_synthesize_for_program(&self, program: &str) -> bool {
        program == "vite"
    }

    async fn synthesize_task(
        &mut self,
        subcommand: CustomTaskSubcommand,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<SyntheticPlanRequest> {
        match subcommand {
            CustomTaskSubcommand::Lint { args } => {
                let direct_execution_cache_key: Arc<[Str]> =
                    iter::once(Str::from("lint")).chain(args.iter().cloned()).collect();
                Ok(SyntheticPlanRequest {
                    program: find_executable_in_node_modules_bin(&*cwd, "oxlint")?,
                    args: args.into(),
                    task_options: Default::default(),
                    direct_execution_cache_key,
                })
            }
        }
    }
}

#[derive(Default)]
pub struct OwnedSessionCallbacks {
    task_synthesizer: TaskSynthesizer,
    user_config_loader: vite_task::loader::JsonUserConfigLoader,
}

impl OwnedSessionCallbacks {
    pub fn as_callbacks(&mut self) -> SessionCallbacks<'_, CustomTaskSubcommand> {
        SessionCallbacks {
            task_synthesizer: &mut self.task_synthesizer,
            user_config_loader: &mut self.user_config_loader,
        }
    }
}

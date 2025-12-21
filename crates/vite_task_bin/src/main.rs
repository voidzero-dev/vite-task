use std::{env::join_paths, ffi::OsStr, path::PathBuf, sync::Arc};

use clap::Parser;
use vite_path::{AbsolutePath, current_dir};
use vite_str::Str;
use vite_task::{CLIArgs, Session, SessionCallbacks, plan_request::SyntheticPlanRequest};

/// This is the custom subcommand that synthesizes tasks for vite-task
#[derive(Debug, Parser)]
enum ViteTaskCustomSubCommand {
    /// oxlint
    Lint { args: Vec<Str> },
}

#[derive(Debug)]
struct TaskSynthesizer;

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
impl vite_task::TaskSynthesizer<ViteTaskCustomSubCommand> for TaskSynthesizer {
    fn should_synthesize_for_program(&self, program: &str) -> bool {
        program == "vite"
    }

    async fn synthesize_task(
        &mut self,
        subcommand: ViteTaskCustomSubCommand,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<SyntheticPlanRequest> {
        match subcommand {
            ViteTaskCustomSubCommand::Lint { args } => Ok(SyntheticPlanRequest {
                program: find_executable_in_node_modules_bin(&*cwd, "oxlint")?,
                args: args.into(),
                task_options: Default::default(),
            }),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    /// This is all the subcommands, including vite-task's own commands (run),
    /// vite-task custom commands (lint), and non-task commands (version)
    #[derive(Debug, Parser)]
    enum Subcommand {
        #[clap(flatten)]
        Task(CLIArgs<ViteTaskCustomSubCommand>),
        Version,
    }

    // Pass vite-task's own/custom subcommands to vite-task's session.
    let task_args = match Subcommand::parse() {
        Subcommand::Task(task_args) => task_args,
        Subcommand::Version => {
            // Non-task subcommands are not handled by vite-task's session.
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
    };

    let mut task_synthesizer = TaskSynthesizer;
    let mut config_loader = vite_task::loader::JsonUserConfigLoader::default();
    let mut session = Session::init(SessionCallbacks {
        task_synthesizer: &mut task_synthesizer,
        user_config_loader: &mut config_loader,
    })?;

    let plan = session.plan(task_args).await?;
    dbg!(plan);

    Ok(())
}

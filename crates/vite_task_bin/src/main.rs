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
enum CustomTaskSubcommand {
    /// oxlint
    Lint { args: Vec<Str> },
}

// These are the subcommands that is not handled by vite-task
#[derive(Debug, Subcommand)]
enum NonTaskSubcommand {
    Version,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd: Arc<AbsolutePath> = current_dir()?.into();
    // Parse the CLI arguments and see if they are for vite-task or not
    let args = match CLIArgs::<CustomTaskSubcommand, NonTaskSubcommand>::try_parse_from(env::args())
    {
        Ok(ok) => ok,
        Err(err) => {
            err.exit();
        }
    };
    let task_cli_args = match args {
        CLIArgs::Task(task_cli_args) => task_cli_args,
        CLIArgs::NonTask(NonTaskSubcommand::Version) => {
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

    let plan = session.plan(cwd, task_cli_args).await?;
    dbg!(plan);

    Ok(())
}

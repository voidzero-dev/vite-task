use std::{env, sync::Arc};

use vite_path::{AbsolutePath, current_dir};
use vite_task::{CLIArgs, Session, SessionCallbacks};
use vite_task_bin::{CustomTaskSubcommand, NonTaskSubcommand, TaskSynthesizer};

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

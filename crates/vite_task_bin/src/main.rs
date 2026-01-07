use std::{env, sync::Arc};

use vite_path::{AbsolutePath, current_dir};
use vite_task::{CLIArgs, Session, SessionCallbacks, session::reporter::LabeledReporter};
use vite_task_bin::{
    CustomTaskSubcommand, NonTaskSubcommand, OwnedSessionCallbacks, TaskSynthesizer,
};

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

    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let mut session = Session::init(owned_callbacks.as_callbacks())?;
    let plan = session.plan_from_cli(cwd, task_cli_args).await?;

    // Create reporter and execute
    let reporter = LabeledReporter::new(std::io::stdout(), session.workspace_path());
    session.execute(plan, Box::new(reporter)).await?;

    Ok(())
}

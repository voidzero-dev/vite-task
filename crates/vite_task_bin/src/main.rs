use std::{env, process::ExitCode, sync::Arc};

use vite_path::{AbsolutePath, current_dir};
use vite_task::{CLIArgs, Session, session::reporter::LabeledReporter};
use vite_task_bin::{CustomTaskSubcommand, NonTaskSubcommand, OwnedSessionCallbacks};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(exit_code) => exit_code,
        Err(err) => {
            eprintln!("Error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<ExitCode> {
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
            return Ok(ExitCode::SUCCESS);
        }
    };

    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let mut session = Session::init(owned_callbacks.as_callbacks())?;
    let plan = session.plan_from_cli(cwd, task_cli_args).await?;

    // Create reporter and execute
    let reporter = LabeledReporter::new(std::io::stdout(), session.workspace_path());
    let exit_code = session.execute(plan, Box::new(reporter)).await;

    Ok(exit_code)
}

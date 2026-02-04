use std::{process::ExitCode, sync::Arc};

use clap::Parser;
use vite_path::{AbsolutePath, current_dir};
use vite_task::{
    BuiltInCommand, Session,
    session::reporter::{ExitStatus, LabeledReporter},
};
use vite_task_bin::OwnedSessionCallbacks;

#[derive(Parser)]
#[command(name = "vite", version)]
struct Cli {
    #[command(subcommand)]
    command: BuiltInCommand,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

async fn run() -> anyhow::Result<ExitStatus> {
    let cwd: Arc<AbsolutePath> = current_dir()?.into();
    let cli = Cli::parse();

    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let mut session = Session::init(owned_callbacks.as_callbacks())?;
    let plan = session.plan_from_cli(cwd, cli.command).await?;

    // Create reporter and execute
    let reporter = LabeledReporter::new(std::io::stdout(), session.workspace_path());
    Ok(session.execute(plan, Box::new(reporter)).await.err().unwrap_or(ExitStatus::SUCCESS))
}

use std::process::ExitCode;

use clap::Parser;
use vite_task::{Command, Session};
use vite_task_bin::OwnedSessionCallbacks;

#[derive(Parser)]
#[command(name = "vite", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

async fn run() -> anyhow::Result<vite_task::ExitStatus> {
    let cli = Cli::parse();
    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let session = Session::init(owned_callbacks.as_callbacks())?;
    session.main(cli.command).await
}

use std::process::ExitCode;

use clap::Parser as _;
use vite_task::{Command, ExitStatus, Session};
use vite_task_bin::OwnedSessionConfig;

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

async fn run() -> anyhow::Result<ExitStatus> {
    let args = Command::parse();
    let mut owned_config = OwnedSessionConfig::default();
    let session = Session::init(owned_config.as_config())?;
    session.main(args).await
}

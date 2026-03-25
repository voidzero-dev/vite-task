use std::process::ExitCode;

use clap::Parser;
use vite_task::{ExitStatus, Session};
use vite_task_bin::{Args, OwnedSessionConfig};

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

async fn run() -> anyhow::Result<ExitStatus> {
    let args = Args::parse();
    let mut owned_config = OwnedSessionConfig::default();
    let session = Session::init(owned_config.as_config())?;
    match args {
        Args::Task(parsed) => session.main(parsed).await,
        args @ Args::Tool { .. } => {
            #[expect(clippy::print_stdout, reason = "CLI binary output for non-task commands")]
            {
                println!("{args:?}");
            }
            Ok(ExitStatus::SUCCESS)
        }
    }
}

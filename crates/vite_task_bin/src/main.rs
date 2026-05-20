use clap::Parser as _;
use vite_task::{Command, ExitStatus, Session};
use vite_task_bin::OwnedSessionConfig;

fn main() -> ! {
    let status: ExitStatus =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(run());

    std::process::exit(i32::from(status.0));
}

async fn run() -> ExitStatus {
    let args = Command::parse();
    let mut owned_config = OwnedSessionConfig::default();
    let session = match Session::init(owned_config.as_config()) {
        Ok(session) => session,
        Err(err) => {
            vite_task::print_error(&err);
            return ExitStatus::FAILURE;
        }
    };
    session.main(args).await
}

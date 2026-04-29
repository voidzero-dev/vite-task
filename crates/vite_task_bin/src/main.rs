use clap::Parser as _;
use vite_task::{Command, ExitStatus, Session};
use vite_task_bin::OwnedSessionConfig;

fn main() -> ! {
    let exit_code: i32 = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async { i32::from(run().await.0) });

    std::process::exit(exit_code);
}

async fn run() -> ExitStatus {
    let args = Command::parse();
    let mut owned_config = OwnedSessionConfig::default();
    match Session::init(owned_config.as_config()) {
        Ok(session) => session.main(args).await,
        #[expect(clippy::print_stderr, reason = "top-level error reporting")]
        Err(err) => {
            eprintln!("Error: {err:?}");
            ExitStatus::FAILURE
        }
    }
}

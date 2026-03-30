use clap::Parser as _;
use vite_task::{Command, ExitStatus, Session};
use vite_task_bin::OwnedSessionConfig;

fn main() -> ! {
    let exit_code: i32 =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(async {
            match run().await {
                Ok(status) => i32::from(status.0),
                #[expect(clippy::print_stderr, reason = "top-level error reporting")]
                Err(err) => {
                    eprintln!("Error: {err:?}");
                    1
                }
            }
        });

    std::process::exit(exit_code);
}

async fn run() -> anyhow::Result<ExitStatus> {
    let args = Command::parse();
    let mut owned_config = OwnedSessionConfig::default();
    let session = Session::init(owned_config.as_config())?;
    session.main(args).await
}

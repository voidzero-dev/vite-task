use vt::{Cli, ExitStatus, Session};
use vt_bin::OwnedSessionConfig;

fn main() -> ! {
    let status: ExitStatus =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(run());

    std::process::exit(i32::from(status.0));
}

async fn run() -> ExitStatus {
    let raw_args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if raw_args.first().is_some_and(|arg| arg == "__complete_word__") {
        let mut owned_config = OwnedSessionConfig::default();
        let request = usage::complete::Request::parse(&raw_args);
        let data = match request {
            Some(request) if vt::completion_uses_workspace_data(&request.split) => {
                match Session::init(owned_config.as_config()) {
                    Ok(mut session) => session.completion_data().await.unwrap_or_default(),
                    Err(_) => vt::CompletionData::default(),
                }
            }
            _ => vt::CompletionData::default(),
        };
        if let Some(answer) = vt::completion_request(&raw_args, &data) {
            use std::io::Write as _;
            let _ = std::io::stdout().write_all(answer.as_bytes());
        }
        return ExitStatus::SUCCESS;
    }

    let command = Cli::parse().command;
    let mut owned_config = OwnedSessionConfig::default();
    let session = match Session::init(owned_config.as_config()) {
        Ok(session) => session,
        Err(err) => {
            vt::print_error(&err);
            return ExitStatus::FAILURE;
        }
    };
    session.main(command).await
}

use std::{process::ExitCode, sync::Arc};

use clap::Parser;
use vite_str::Str;
use vite_task::{
    EnabledCacheConfig, ExitStatus, Session, UserCacheConfig, UserTaskOptions, get_path_env,
    plan_request::SyntheticPlanRequest,
};
use vite_task_bin::{Args, OwnedSessionCallbacks, find_executable};

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

async fn run() -> anyhow::Result<ExitStatus> {
    let args = Args::parse();
    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let session = Session::init(owned_callbacks.as_callbacks())?;
    match args {
        Args::Task(command) => session.main(command).await,
        args => {
            // If env FOO is set, run `print-env FOO` via Session::exec before proceeding.
            // In vite-plus, Session::exec is used for auto-install.
            let envs = session.envs();
            if envs.contains_key(std::ffi::OsStr::new("FOO")) {
                let program = find_executable(get_path_env(envs), session.cwd(), "print-env")?;
                let request = SyntheticPlanRequest {
                    program,
                    args: [Str::from("FOO")].into(),
                    task_options: UserTaskOptions {
                        cache_config: UserCacheConfig::Enabled {
                            cache: None,
                            enabled_cache_config: EnabledCacheConfig {
                                envs: Some(Box::from([Str::from("FOO")])),
                                pass_through_envs: None,
                            },
                        },
                        ..Default::default()
                    },
                    envs: Arc::clone(envs),
                };
                let cache_key: Arc<[Str]> = Arc::from([Str::from("print-env-foo")]);
                let status = session.execute_synthetic(request, cache_key, true).await?;
                if status != ExitStatus::SUCCESS {
                    return Ok(status);
                }
            }
            println!("{:?}", args);
            Ok(ExitStatus::SUCCESS)
        }
    }
}

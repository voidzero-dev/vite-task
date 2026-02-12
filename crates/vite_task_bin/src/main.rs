use std::{
    io::{IsTerminal, Read, Write},
    process::ExitCode,
    sync::Arc,
};

use clap::Parser;
use vite_str::Str;
use vite_task::{
    EnabledCacheConfig, ExitStatus, Session, UserCacheConfig, get_path_env,
    plan_request::SyntheticPlanRequest,
};
use vite_task_bin::{Args, OwnedSessionCallbacks, find_executable};

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    #[expect(clippy::large_futures, reason = "top-level await in main, no alternative")]
    let exit_status = run().await?;
    Ok(exit_status.0.into())
}

#[expect(clippy::future_not_send, reason = "Session contains !Send types; single-threaded runtime")]
async fn run() -> anyhow::Result<ExitStatus> {
    let args = Args::parse();
    let mut owned_callbacks = OwnedSessionCallbacks::default();
    let session = Session::init(owned_callbacks.as_callbacks())?;
    match args {
        Args::Interact => run_interact(),
        Args::Task(command) => {
            #[expect(clippy::large_futures, reason = "session.main produces a large future")]
            {
                session.main(command).await
            }
        }
        args => {
            // If env FOO is set, run `print-env FOO` via Session::exec before proceeding.
            // In vite-plus, Session::exec is used for auto-install.
            let envs = session.envs();
            if envs.contains_key(std::ffi::OsStr::new("FOO")) {
                let program = find_executable(get_path_env(envs), session.cwd(), "print-env")?;
                let request = SyntheticPlanRequest {
                    program,
                    args: [Str::from("FOO")].into(),
                    cache_config: UserCacheConfig::with_config({
                        EnabledCacheConfig {
                            envs: Some(Box::from([Str::from("FOO")])),
                            pass_through_envs: None,
                        }
                    }),
                    envs: Arc::clone(envs),
                };
                let cache_key: Arc<[Str]> = Arc::from([Str::from("print-env-foo")]);
                #[expect(
                    clippy::large_futures,
                    reason = "execute_synthetic produces a large future"
                )]
                let status = session.execute_synthetic(request, cache_key, true).await?;
                if status != ExitStatus::SUCCESS {
                    return Ok(status);
                }
            }
            #[expect(clippy::print_stdout, reason = "CLI binary output for non-task commands")]
            {
                println!("{args:?}");
            }
            Ok(ExitStatus::SUCCESS)
        }
    }
}

fn write_line(stdout: &mut impl Write, line: &[u8]) -> anyhow::Result<()> {
    stdout.write_all(line)?;
    stdout.write_all(b"\r\n")?;
    stdout.flush()?;
    Ok(())
}

fn write_milestone(stdout: &mut impl Write, name: &str) -> anyhow::Result<()> {
    stdout.write_all(&pty_terminal_test_client::encoded_milestone(name))?;
    stdout.flush()?;
    Ok(())
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn new(enabled: bool) -> anyhow::Result<Self> {
        if enabled {
            crossterm::terminal::enable_raw_mode()?;
        }
        Ok(Self { enabled })
    }

    fn disable(&mut self) -> anyhow::Result<()> {
        if self.enabled {
            crossterm::terminal::disable_raw_mode()?;
            self.enabled = false;
        }
        Ok(())
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

fn run_interact() -> anyhow::Result<ExitStatus> {
    let stdin_is_tty = std::io::stdin().is_terminal();
    let mut raw_mode = RawModeGuard::new(stdin_is_tty)?;

    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut text_buffer = Vec::<u8>::new();

    write_line(&mut stdout, b"START")?;
    write_milestone(&mut stdout, "ready")?;

    loop {
        let mut byte = [0u8; 1];
        let read_count = stdin.read(&mut byte)?;
        if read_count == 0 {
            break;
        }

        let byte = byte[0];
        if byte == 0x1b {
            let mut seq = [0u8; 2];
            if stdin.read_exact(&mut seq).is_err() {
                break;
            }

            if seq == [b'[', b'A'] {
                write_line(&mut stdout, b"KEY:UP")?;
                write_milestone(&mut stdout, "after-up")?;
            } else if seq == [b'[', b'B'] {
                write_line(&mut stdout, b"KEY:DOWN")?;
                write_milestone(&mut stdout, "after-down")?;
            }
            continue;
        }

        if byte == b'\r' {
            if text_buffer.is_empty() {
                write_line(&mut stdout, b"KEY:ENTER")?;
                raw_mode.disable()?;
                write_line(&mut stdout, b"DONE")?;
                write_milestone(&mut stdout, "after-enter")?;
                return Ok(ExitStatus::SUCCESS);
            }

            stdout.write_all(b"LINE:")?;
            stdout.write_all(&text_buffer)?;
            stdout.write_all(b"\r\n")?;
            stdout.flush()?;
            text_buffer.clear();
            write_milestone(&mut stdout, "after-line")?;
            continue;
        }

        if byte == b'\n' {
            if !text_buffer.is_empty() {
                stdout.write_all(b"LINE:")?;
                stdout.write_all(&text_buffer)?;
                stdout.write_all(b"\r\n")?;
                stdout.flush()?;
                text_buffer.clear();
                write_milestone(&mut stdout, "after-line")?;
            }
            continue;
        }

        text_buffer.push(byte);
        stdout.write_all(b"CHAR:")?;
        stdout.write_all(&[byte])?;
        stdout.write_all(b"\r\n")?;
        stdout.flush()?;
    }

    Ok(ExitStatus::SUCCESS)
}

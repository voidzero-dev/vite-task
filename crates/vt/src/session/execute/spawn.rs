//! Unified spawn abstraction over fspy and plain tokio processes.
//!
//! [`spawn`] does one thing: hand back the child's stdio pipes plus a
//! cancellation-aware `wait` future. Draining the pipes is [`super::pipe`]'s
//! job; normalizing fspy path accesses is [`super::tracked_accesses`]'s (only
//! compiled when `cfg(fspy)` is on).

use std::{ffi::OsStr, io, process::Stdio};

#[cfg(fspy)]
use fspy::PathAccessIterable;
use futures_util::{FutureExt, future::LocalBoxFuture};
use tokio::process::{ChildStderr, ChildStdout};
use tokio_util::sync::CancellationToken;
use vt_plan::SpawnCommand;

/// How the child's stdin/stdout/stderr are configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnStdio {
    /// All three fds inherited from the parent. On Unix, [`spawn`] also clears
    /// `FD_CLOEXEC` on fds 0-2 (libuv workaround —
    /// <https://github.com/libuv/libuv/issues/2062>).
    Inherited,
    /// stdin is `/dev/null`; stdout and stderr are piped. Drain the pipes with
    /// [`super::pipe::pipe_stdio`].
    Piped,
}

/// Handle to a spawned child.
///
/// `stdout` and `stderr` are `Some` iff [`SpawnStdio::Piped`] was requested.
/// `wait` resolves when the child exits and handles cancellation internally:
/// when the token fires, noninteractive tasks terminate their owned process group
/// (or Windows Job Object). Interactive Unix tasks keep the terminal foreground
/// group and its existing signal delivery.
pub struct ChildHandle {
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    pub wait: LocalBoxFuture<'static, io::Result<ChildOutcome>>,
}

/// Result of waiting for a child to exit.
pub struct ChildOutcome {
    pub exit_status: std::process::ExitStatus,
    /// Raw fspy accesses. `Some` iff `fspy` was `true` at spawn time, and
    /// `Err` when a tracked process could not record everything it did.
    #[cfg(fspy)]
    pub path_accesses: Option<Result<PathAccessIterable, fspy::TrackingIncomplete>>,
}

/// Spawn a command with the requested fspy and stdio configuration.
///
/// `extra_envs` are applied **after** `cmd.spawn_envs`, so runtime-injected
/// entries (e.g. the runner's IPC name + napi addon path) override any
/// same-named key from the plan.
///
/// Cancellation is unified: whether fspy is enabled or not, the returned `wait`
/// future observes `cancellation_token` and kills the child before resolving.
///
/// On builds without `cfg(fspy)`, the `fspy` argument is ignored and the tokio
/// path is always taken.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn spawn<E, K, V>(
    cmd: &SpawnCommand,
    fspy: bool,
    stdio: SpawnStdio,
    cancellation_token: CancellationToken,
    interrupt_token: CancellationToken,
    extra_envs: E,
) -> anyhow::Result<ChildHandle>
where
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    #[cfg(fspy)]
    if fspy {
        return spawn_fspy(cmd, stdio, cancellation_token, interrupt_token, extra_envs).await;
    }
    #[cfg(not(fspy))]
    let _ = fspy;

    let mut tokio_cmd = tokio::process::Command::new(cmd.program_path.as_path());
    tokio_cmd.args(cmd.args.iter().map(vt_str::Str::as_str));
    tokio_cmd.env_clear();
    tokio_cmd.envs(cmd.spawn_envs.iter());
    tokio_cmd.envs(extra_envs);
    tokio_cmd.current_dir(&*cmd.cwd);
    apply_stdio(&mut tokio_cmd, stdio);
    spawn_tokio(tokio_cmd, stdio, cancellation_token, interrupt_token)
}

#[cfg(fspy)]
async fn spawn_fspy<E, K, V>(
    cmd: &SpawnCommand,
    stdio: SpawnStdio,
    cancellation_token: CancellationToken,
    interrupt_token: CancellationToken,
    extra_envs: E,
) -> anyhow::Result<ChildHandle>
where
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut fspy_cmd = fspy::Command::new(cmd.program_path.as_path());
    fspy_cmd.args(cmd.args.iter().map(vt_str::Str::as_str));
    fspy_cmd.envs(cmd.spawn_envs.iter());
    fspy_cmd.envs(extra_envs);
    fspy_cmd.current_dir(&*cmd.cwd);

    match stdio {
        SpawnStdio::Inherited => {
            fspy_cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
            // libuv (used by Node.js) marks stdin/stdout/stderr as close-on-exec;
            // without this fix the child reopens fds 0-2 as /dev/null after exec.
            // See: https://github.com/libuv/libuv/issues/2062
            // SAFETY: the pre_exec closure only performs fcntl operations on
            // stdio fds, which is safe in a post-fork context.
            #[cfg(unix)]
            unsafe {
                fspy_cmd.pre_exec(clear_stdio_cloexec);
            }
        }
        SpawnStdio::Piped => {
            fspy_cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }

    #[cfg(unix)]
    let group = isolate_process_group(stdio);
    #[cfg(unix)]
    if group {
        fspy_cmd.process_group(0);
    }
    // Task ownership includes descendants. Keep cancellation here so the
    // whole task scope terminates before the trace is collected.
    let mut tracked = fspy_cmd.spawn(CancellationToken::new()).await?;
    #[cfg(unix)]
    let process_scope = TaskProcess {
        id: nix::unistd::Pid::from_raw(tracked.id.try_into().expect("process ID fits pid_t")),
        group,
    };

    // On Windows, assign the child to a Job Object so that killing the child
    // also kills all descendant processes (e.g., node.exe via a .cmd shim).
    #[cfg(windows)]
    let process_scope = TaskProcess {
        job: {
            use std::os::windows::io::AsRawHandle;
            super::win_job::assign_to_kill_on_close_job(tracked.process_handle.as_raw_handle())?
        },
    };

    let stdout = tracked.stdout.take();
    let stderr = tracked.stderr.take();
    let wait_handle = tracked.wait_handle;

    let wait = async move {
        let termination =
            wait_for_termination(wait_handle, process_scope, cancellation_token, interrupt_token)
                .await?;
        Ok(ChildOutcome {
            exit_status: termination.status,
            path_accesses: Some(termination.path_accesses),
        })
    }
    .boxed_local();

    Ok(ChildHandle { stdout, stderr, wait })
}

fn spawn_tokio(
    mut cmd: tokio::process::Command,
    stdio: SpawnStdio,
    cancellation_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> anyhow::Result<ChildHandle> {
    #[cfg(unix)]
    let group = isolate_process_group(stdio);
    #[cfg(unix)]
    if group {
        cmd.process_group(0);
    }
    #[cfg(windows)]
    let _ = stdio;
    let mut child = cmd.spawn()?;
    #[cfg(unix)]
    let process_scope = TaskProcess {
        id: nix::unistd::Pid::from_raw(
            child
                .id()
                .expect("new child has a process ID")
                .try_into()
                .expect("process ID fits pid_t"),
        ),
        group,
    };

    #[cfg(windows)]
    let process_scope = TaskProcess {
        job: {
            use std::os::windows::io::{AsRawHandle, BorrowedHandle};
            // Duplicate the process handle so the job outlives tokio's handle.
            // SAFETY: The child was just spawned, so its raw handle is valid.
            let borrowed = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
            let owned = borrowed.try_clone_to_owned()?;
            super::win_job::assign_to_kill_on_close_job(owned.as_raw_handle())?
        },
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let wait = async move {
        let exit_status = wait_for_termination(
            async move { child.wait().await },
            process_scope,
            cancellation_token,
            interrupt_token,
        )
        .await?;
        Ok(ChildOutcome {
            exit_status,
            #[cfg(fspy)]
            path_accesses: None,
        })
    }
    .boxed_local();

    Ok(ChildHandle { stdout, stderr, wait })
}

#[cfg(unix)]
fn isolate_process_group(stdio: SpawnStdio) -> bool {
    use std::io::IsTerminal;
    stdio == SpawnStdio::Piped || !std::io::stdin().is_terminal()
}

struct TaskProcess {
    #[cfg(unix)]
    id: nix::unistd::Pid,
    #[cfg(unix)]
    group: bool,
    #[cfg(windows)]
    job: super::win_job::OwnedJobHandle,
}

impl TaskProcess {
    const fn forwards_interrupt(&self) -> bool {
        #[cfg(unix)]
        {
            self.group
        }
        #[cfg(windows)]
        {
            false
        }
    }

    fn terminate(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.signal(nix::sys::signal::Signal::SIGKILL)
        }
        #[cfg(windows)]
        {
            self.job.terminate()
        }
    }

    #[cfg(unix)]
    fn interrupt(&self) -> io::Result<()> {
        self.signal(nix::sys::signal::Signal::SIGINT)
    }

    #[cfg(unix)]
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        use nix::{
            errno::Errno,
            sys::signal::{kill, killpg},
        };
        // Only piped or noninteractive tasks enter a fresh group. Never signal the
        // runner's own foreground group or a group discovered by enumeration.
        let result = if self.group { killpg(self.id, signal) } else { kill(self.id, signal) };
        match result {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

async fn wait_for_termination<T>(
    termination: impl std::future::Future<Output = io::Result<T>>,
    process_scope: TaskProcess,
    cancellation_token: CancellationToken,
    interrupt_token: CancellationToken,
) -> io::Result<T> {
    tokio::pin!(termination);
    let mut interrupted = false;
    loop {
        tokio::select! {
            result = &mut termination => return result,
            () = cancellation_token.cancelled() => {
                process_scope.terminate()?;
                return termination.await;
            }
            () = interrupt_token.cancelled(), if process_scope.forwards_interrupt() && !interrupted => {
                #[cfg(unix)]
                process_scope.interrupt()?;
                interrupted = true;
            }
        }
    }
}

fn apply_stdio(cmd: &mut tokio::process::Command, stdio: SpawnStdio) {
    match stdio {
        SpawnStdio::Inherited => {
            cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
            // libuv (used by Node.js) marks stdin/stdout/stderr as close-on-exec;
            // without this fix the child reopens fds 0-2 as /dev/null after exec.
            // See: https://github.com/libuv/libuv/issues/2062
            // SAFETY: the pre_exec closure only performs fcntl operations on
            // stdio fds, which is safe in a post-fork context.
            #[cfg(unix)]
            unsafe {
                cmd.pre_exec(clear_stdio_cloexec);
            }
        }
        SpawnStdio::Piped => {
            cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }
}

#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "signature matches Command::pre_exec's FnMut() -> io::Result<()> contract"
)]
fn clear_stdio_cloexec() -> io::Result<()> {
    use std::os::fd::BorrowedFd;

    use nix::{
        fcntl::{FcntlArg, FdFlag, fcntl},
        libc::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO},
    };
    for fd in [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO] {
        // SAFETY: fds 0-2 are always valid in a post-fork context
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        if let Ok(flags) = fcntl(borrowed, FcntlArg::F_GETFD) {
            let mut fd_flags = FdFlag::from_bits_retain(flags);
            if fd_flags.contains(FdFlag::FD_CLOEXEC) {
                fd_flags.remove(FdFlag::FD_CLOEXEC);
                let _ = fcntl(borrowed, FcntlArg::F_SETFD(fd_flags));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{io, sync::Arc, time::Duration};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use tokio_util::sync::CancellationToken;
    use vt_path::AbsolutePath;
    use vt_plan::SpawnCommand;

    use super::{SpawnStdio, spawn};

    // https://github.com/voidzero-dev/vite-task/pull/675
    // Nonblocking trace collection must not leave cancelled task descendants alive.
    #[tokio::test]
    async fn cancelled_task_terminates_descendants() -> anyhow::Result<()> {
        let mut failures = Vec::new();
        for tracked in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let address = vt_str::Str::from(listener.local_addr()?.to_string());
            let command = subprocess_test::command_for_fn!(address, |address: vt_str::Str| {
                use std::io::Read;
                // Do not spawn descendants until the runner has finished
                // attaching the task's scope (including Windows Job Objects).
                let mut started = std::net::TcpStream::connect(address.as_str()).unwrap();
                started.read_exact(&mut [0u8]).unwrap();
                drop(started);
                let descendant =
                    subprocess_test::command_for_fn!(address, |address: vt_str::Str| {
                        use std::io::{Read, Write};
                        let mut stream = std::net::TcpStream::connect(address.as_str()).unwrap();
                        stream.write_all(&std::process::id().to_ne_bytes()).unwrap();
                        // Only the owning test can release this barrier. Cancellation
                        // must terminate the descendant without releasing it.
                        let _ = stream.read_exact(&mut [0u8]);
                    });
                let mut command = std::process::Command::from(descendant);
                command.stdin(std::process::Stdio::null());
                command.stdout(std::process::Stdio::null());
                command.stderr(std::process::Stdio::null());
                command.spawn().unwrap().wait().unwrap();
            });
            let command = SpawnCommand {
                program_path: Arc::from(AbsolutePath::new(&command.program).unwrap()),
                args: command.args.iter().map(|arg| arg.to_str().unwrap().into()).collect(),
                spawn_envs: Arc::new(
                    command.envs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
                ),
                cwd: Arc::from(AbsolutePath::new(&command.cwd).unwrap()),
            };
            let cancelled = CancellationToken::new();
            let mut child = spawn(
                &command,
                tracked,
                SpawnStdio::Piped,
                cancelled.clone(),
                CancellationToken::new(),
                std::iter::empty::<(&str, &str)>(),
            )
            .await?;
            let (mut started, _) = listener.accept().await?;
            started.write_all(b"x").await?;
            drop(started);
            let (mut stream, _) = listener.accept().await?;
            let mut pid = [0u8; 4];
            stream.read_exact(&mut pid).await?;
            let descendant_pid = u32::from_ne_bytes(pid);
            cancelled.cancel();

            let mut outcome = None;
            let mut byte = [0u8];
            let settled = tokio::time::timeout(Duration::from_secs(2), async {
                let (eof, status) = tokio::join!(stream.read(&mut byte), async {
                    outcome = Some(child.wait.as_mut().await?);
                    io::Result::Ok(())
                });
                status?;
                assert_eq!(eof?, 0, "descendant barrier was unexpectedly released");
                io::Result::Ok(())
            })
            .await;
            if let Ok(result) = settled {
                result?;
            } else {
                // Clean up the known descendant on the red implementation.
                // This release is never used to satisfy the assertion.
                stream.write_all(b"x").await?;
                if outcome.is_none() {
                    outcome = Some(child.wait.await?);
                }
                failures.push(vt_str::format!(
                    "tracking={tracked}: cancellation left descendant {descendant_pid} alive"
                ));
            }
            assert!(!outcome.unwrap().exit_status.success());
        }
        assert!(failures.is_empty(), "{failures:?}");
        Ok(())
    }
}

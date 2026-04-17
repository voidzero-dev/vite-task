//! Unified spawn abstraction over fspy and plain tokio processes.
//!
//! [`spawn`] does one thing: hand back the child's stdio pipes plus a
//! cancellation-aware `wait` future. Draining the pipes is [`super::pipe`]'s
//! job; normalizing fspy path accesses is [`super::tracked_accesses`]'s.

use std::{io, process::Stdio};

use fspy::PathAccessIterable;
use futures_util::{FutureExt, future::LocalBoxFuture};
use tokio::process::{ChildStderr, ChildStdout};
use tokio_util::sync::CancellationToken;
use vite_task_plan::SpawnCommand;

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
/// when the token fires, the child (and on Windows its descendants via the Job
/// Object) is killed before the future resolves.
pub struct ChildHandle {
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
    pub wait: LocalBoxFuture<'static, io::Result<ChildOutcome>>,
}

/// Result of waiting for a child to exit.
pub struct ChildOutcome {
    pub exit_status: std::process::ExitStatus,
    /// Raw fspy accesses. `Some` iff `fspy` was `true` at spawn time.
    pub path_accesses: Option<PathAccessIterable>,
}

/// Spawn a command with the requested fspy and stdio configuration.
///
/// Cancellation is unified: whether fspy is enabled or not, the returned `wait`
/// future observes `cancellation_token` and kills the child before resolving.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn spawn(
    cmd: &SpawnCommand,
    fspy: bool,
    stdio: SpawnStdio,
    cancellation_token: CancellationToken,
) -> anyhow::Result<ChildHandle> {
    let mut fspy_cmd = fspy::Command::new(cmd.program_path.as_path());
    fspy_cmd.args(cmd.args.iter().map(vite_str::Str::as_str));
    fspy_cmd.envs(cmd.all_envs.iter());
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

    if fspy {
        spawn_fspy(fspy_cmd, cancellation_token).await
    } else {
        spawn_tokio(fspy_cmd, cancellation_token)
    }
}

async fn spawn_fspy(
    cmd: fspy::Command,
    cancellation_token: CancellationToken,
) -> anyhow::Result<ChildHandle> {
    let mut tracked = cmd.spawn(cancellation_token).await?;

    // On Windows, assign the child to a Job Object so that killing the child
    // also kills all descendant processes (e.g., node.exe via a .cmd shim).
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        super::win_job::assign_to_kill_on_close_job(tracked.process_handle.as_raw_handle())?
    };

    let stdout = tracked.stdout.take();
    let stderr = tracked.stderr.take();
    let wait_handle = tracked.wait_handle;

    let wait = async move {
        let termination = wait_handle.await?;
        // Drop order: `job` drops here, KILL_ON_JOB_CLOSE kills any descendants
        // still alive. fspy's wait handle already watched the cancellation
        // token and killed the direct child.
        #[cfg(windows)]
        drop(job);
        Ok(ChildOutcome {
            exit_status: termination.status,
            path_accesses: Some(termination.path_accesses),
        })
    }
    .boxed_local();

    Ok(ChildHandle { stdout, stderr, wait })
}

fn spawn_tokio(
    cmd: fspy::Command,
    cancellation_token: CancellationToken,
) -> anyhow::Result<ChildHandle> {
    let mut child = cmd.into_tokio_command().spawn()?;

    #[cfg(windows)]
    let job = {
        use std::os::windows::io::{AsRawHandle, BorrowedHandle};
        // Duplicate the process handle so the job outlives tokio's handle.
        // SAFETY: The child was just spawned, so its raw handle is valid.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
        let owned = borrowed.try_clone_to_owned()?;
        super::win_job::assign_to_kill_on_close_job(owned.as_raw_handle())?
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let wait = async move {
        let exit_status = tokio::select! {
            status = child.wait() => status?,
            () = cancellation_token.cancelled() => {
                child.start_kill()?;
                // Eagerly kill descendants; KILL_ON_JOB_CLOSE on drop is a backstop.
                #[cfg(windows)]
                job.terminate();
                child.wait().await?
            }
        };
        // `job` drops here on Windows, terminating any stragglers.
        #[cfg(windows)]
        drop(job);
        Ok(ChildOutcome { exit_status, path_accesses: None })
    }
    .boxed_local();

    Ok(ChildHandle { stdout, stderr, wait })
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

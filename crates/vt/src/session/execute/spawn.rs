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

use crate::session::watch::inputs::LeafWatch;

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
    extra_envs: E,
    watch: Option<&LeafWatch>,
) -> anyhow::Result<ChildHandle>
where
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let nested_watch = watch.is_some_and(|watch| watch.nested_watch);
    // A persistent child runner establishes its own tracing session. Injecting
    // this session's preload into it would conflict with its children's trace.
    #[cfg(fspy)]
    if !nested_watch && (fspy || watch.is_some()) {
        return spawn_fspy(cmd, stdio, cancellation_token, extra_envs, watch).await;
    }
    #[cfg(not(fspy))]
    let _ = fspy;
    #[cfg(not(fspy))]
    if watch.is_some() {
        anyhow::bail!("Watch mode requires file access tracing on this platform");
    }

    let mut tokio_cmd = tokio::process::Command::new(cmd.program_path.as_path());
    tokio_cmd.args(cmd.args.iter().map(vt_str::Str::as_str));
    tokio_cmd.env_clear();
    tokio_cmd.envs(cmd.spawn_envs.iter());
    tokio_cmd.envs(extra_envs);
    tokio_cmd.current_dir(&*cmd.cwd);
    apply_stdio(&mut tokio_cmd, stdio);
    spawn_tokio(tokio_cmd, cancellation_token, nested_watch)
}

#[cfg(fspy)]
async fn spawn_fspy<E, K, V>(
    cmd: &SpawnCommand,
    stdio: SpawnStdio,
    cancellation_token: CancellationToken,
    extra_envs: E,
    watch: Option<&LeafWatch>,
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
    if let Some(watch) = watch {
        fspy_cmd.observe_accesses(watch.observer()).kill_process_tree(true);
    }

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

    let mut tracked = fspy_cmd.spawn(cancellation_token.clone()).await?;

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
        tokio::pin!(wait_handle);
        let termination = tokio::select! {
            result = &mut wait_handle => result?,
            () = cancellation_token.cancelled() => {
                #[cfg(windows)]
                job.terminate();
                wait_handle.await?
            }
        };
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
    mut cmd: tokio::process::Command,
    cancellation_token: CancellationToken,
    process_tree: bool,
) -> anyhow::Result<ChildHandle> {
    #[cfg(unix)]
    if process_tree {
        cmd.process_group(0);
    }
    #[cfg(unix)]
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    let (mut child, job) = spawn_in_job(&mut cmd)?;
    #[cfg(windows)]
    let _ = process_tree;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    #[cfg(unix)]
    let process_id = child.id();
    let wait = async move {
        let exit_status = tokio::select! {
            status = child.wait() => status?,
            () = cancellation_token.cancelled() => {
                #[cfg(unix)]
                if process_tree {
                    interrupt_group(process_id);
                    // The nested runner must finish reaping its separate task
                    // groups before we reap it. Bound unresponsive runners.
                    if tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await.is_err() {
                        kill_group(process_id);
                    }
                }
                child.start_kill()?;
                // Eagerly kill descendants; KILL_ON_JOB_CLOSE on drop is a backstop.
                #[cfg(windows)]
                job.terminate();
                child.wait().await?
            }
        };
        #[cfg(unix)]
        if process_tree {
            kill_group(process_id);
        }
        // `job` drops here on Windows, terminating any stragglers.
        #[cfg(windows)]
        drop(job);
        Ok(ChildOutcome {
            exit_status,
            #[cfg(fspy)]
            path_accesses: None,
        })
    }
    .boxed_local();

    Ok(ChildHandle { stdout, stderr, wait })
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

#[cfg(unix)]
fn interrupt_group(id: Option<u32>) {
    if let Some(id) = id.and_then(|id| i32::try_from(id).ok()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(id),
            nix::sys::signal::Signal::SIGINT,
        );
    }
}

#[cfg(unix)]
fn kill_group(id: Option<u32>) {
    if let Some(id) = id.and_then(|id| i32::try_from(id).ok()) {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(id),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

#[cfg(windows)]
fn spawn_in_job(
    cmd: &mut tokio::process::Command,
) -> io::Result<(tokio::process::Child, super::win_job::OwnedJobHandle)> {
    use std::os::windows::{io::AsRawHandle as _, process::ChildExt as _};

    use winapi::um::{processthreadsapi::ResumeThread, winbase::CREATE_SUSPENDED};
    cmd.creation_flags(CREATE_SUSPENDED);
    let mut job = None;
    let child = cmd.spawn_with(|cmd| {
        let mut child = cmd.spawn()?;
        let result = (|| {
            job = Some(super::win_job::assign_to_kill_on_close_job(child.as_raw_handle())?);
            // SAFETY: this is the suspended primary thread of the new process.
            if unsafe { ResumeThread(child.main_thread_handle().as_raw_handle().cast()) }
                == u32::MAX
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    })?;
    Ok((child, job.expect("spawned child has a job")))
}

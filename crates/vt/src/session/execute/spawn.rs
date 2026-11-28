//! Unified spawn abstraction over fspy and plain tokio processes.
//!
//! Two entry points enforce valid stdio/tracking combinations at compile time:
//! - [`spawn_piped`] pipes stdout/stderr (optionally under fspy tracing) and
//!   returns [`PipedChild`] with coupled [`ChildPipes`].
//! - [`spawn_inherited`] inherits stdio directly (never fspy-tracked) and
//!   returns [`InheritedChild`] with no pipes.
//!
//! Draining pipes is [`super::pipe`]'s job; normalizing fspy path accesses is
//! [`super::tracked_accesses`]'s (only compiled when `cfg(fspy)` is on).

use std::{ffi::OsStr, io, process::Stdio};

#[cfg(fspy)]
use fspy::PathAccessIterable;
use futures_util::{FutureExt, future::LocalBoxFuture};
use tokio::process::{ChildStderr, ChildStdout};
use tokio_util::sync::CancellationToken;
use vt_plan::SpawnCommand;

/// Whether a piped child runs under fspy filesystem tracing.
///
/// Inherited stdio cannot be combined with fspy tracing, so tracking is only
/// accepted by [`spawn_piped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipedMode {
    /// Plain tokio process without filesystem tracing.
    Plain,
    /// Tracked with fspy when `cfg(fspy)` is enabled; falls back to a plain
    /// tokio process on targets without `cfg(fspy)`.
    Tracked,
}

/// Coupled stdout/stderr pipes from a piped child process.
///
/// Both pipes are always created together, so half-piped states are
/// unrepresentable.
pub struct ChildPipes {
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// Cancellation-aware future resolving when the child exits.
///
/// When the cancellation token fires, the child (and on Windows its
/// descendants via the Job Object) is killed before the future resolves.
pub type ChildWait = LocalBoxFuture<'static, io::Result<ChildOutcome>>;

/// Handle to a child spawned with piped stdout/stderr.
pub struct PipedChild {
    pub pipes: ChildPipes,
    pub wait: ChildWait,
}

/// Handle to a child spawned with inherited stdio.
pub struct InheritedChild {
    pub wait: ChildWait,
}

/// Result of waiting for a child to exit.
pub struct ChildOutcome {
    pub exit_status: std::process::ExitStatus,
    /// Raw fspy accesses. `Some` iff spawned with [`PipedMode::Tracked`], and
    /// `Err` when complete accesses could not be collected (incomplete or nested).
    #[cfg(fspy)]
    pub path_accesses: Option<Result<PathAccessIterable, fspy::UntrackedReason>>,
}

/// Spawn a command with piped stdout/stderr (`stdin` set to `/dev/null`) and
/// optional fspy filesystem tracking.
///
/// `extra_envs` are applied **after** `cmd.spawn_envs`, so runtime-injected
/// entries (e.g. the runner's IPC name + napi addon path) override any
/// same-named key from the plan.
///
/// Cancellation is unified: whether fspy is enabled or not, the returned
/// [`PipedChild::wait`] future observes `cancellation_token` and kills the
/// child before resolving.
///
/// On builds without `cfg(fspy)`, [`PipedMode::Tracked`] falls back to the
/// plain tokio path.
#[tracing::instrument(level = "debug", skip_all)]
pub async fn spawn_piped<E, K, V>(
    cmd: &SpawnCommand,
    mode: PipedMode,
    cancellation_token: CancellationToken,
    extra_envs: E,
) -> anyhow::Result<PipedChild>
where
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    #[cfg(fspy)]
    if mode == PipedMode::Tracked {
        return spawn_fspy(cmd, cancellation_token, extra_envs).await;
    }
    #[cfg(not(fspy))]
    let _ = mode;

    let mut tokio_cmd = build_tokio_command(cmd, extra_envs);
    tokio_cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = tokio_cmd.spawn()?;
    let pipes = take_pipes(&mut child.stdout, &mut child.stderr);
    let wait = spawn_tokio_wait(child, cancellation_token)?;
    Ok(PipedChild { pipes, wait })
}

/// Spawn a command with stdin/stdout/stderr inherited from the parent.
///
/// Inherited spawns never run under fspy tracking. On Unix, this also clears
/// `FD_CLOEXEC` on fds 0-2 (libuv workaround —
/// <https://github.com/libuv/libuv/issues/2062>).
#[tracing::instrument(level = "debug", skip_all)]
pub fn spawn_inherited(
    cmd: &SpawnCommand,
    cancellation_token: CancellationToken,
) -> anyhow::Result<InheritedChild> {
    let mut tokio_cmd = build_tokio_command(cmd, std::iter::empty::<(&OsStr, &OsStr)>());
    tokio_cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    // libuv (used by Node.js) marks stdin/stdout/stderr as close-on-exec;
    // without this fix the child reopens fds 0-2 as /dev/null after exec.
    // See: https://github.com/libuv/libuv/issues/2062
    // SAFETY: the pre_exec closure only performs fcntl operations on
    // stdio fds, which is safe in a post-fork context.
    #[cfg(unix)]
    unsafe {
        tokio_cmd.pre_exec(clear_stdio_cloexec);
    }
    let child = tokio_cmd.spawn()?;
    let wait = spawn_tokio_wait(child, cancellation_token)?;
    Ok(InheritedChild { wait })
}

#[cfg(fspy)]
async fn spawn_fspy<E, K, V>(
    cmd: &SpawnCommand,
    cancellation_token: CancellationToken,
    extra_envs: E,
) -> anyhow::Result<PipedChild>
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
    fspy_cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut tracked = fspy_cmd.spawn(cancellation_token).await?;

    // On Windows, assign the child to a Job Object so that killing the child
    // also kills all descendant processes (e.g., node.exe via a .cmd shim).
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::AsRawHandle;
        super::win_job::assign_to_kill_on_close_job(tracked.process_handle.as_raw_handle())?
    };

    let pipes = take_pipes(&mut tracked.stdout, &mut tracked.stderr);
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

    Ok(PipedChild { pipes, wait })
}

const fn take_pipes(
    stdout: &mut Option<ChildStdout>,
    stderr: &mut Option<ChildStderr>,
) -> ChildPipes {
    ChildPipes {
        stdout: stdout.take().expect("stdout was configured as Stdio::piped()"),
        stderr: stderr.take().expect("stderr was configured as Stdio::piped()"),
    }
}

fn build_tokio_command<E, K, V>(cmd: &SpawnCommand, extra_envs: E) -> tokio::process::Command
where
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut tokio_cmd = tokio::process::Command::new(cmd.program_path.as_path());
    tokio_cmd.args(cmd.args.iter().map(vt_str::Str::as_str));
    tokio_cmd.env_clear();
    tokio_cmd.envs(cmd.spawn_envs.iter());
    tokio_cmd.envs(extra_envs);
    tokio_cmd.current_dir(&*cmd.cwd);
    tokio_cmd
}

#[cfg_attr(
    not(windows),
    expect(clippy::unnecessary_wraps, reason = "Windows job object assignment is fallible")
)]
fn spawn_tokio_wait(
    mut child: tokio::process::Child,
    cancellation_token: CancellationToken,
) -> anyhow::Result<ChildWait> {
    #[cfg(windows)]
    let job = {
        use std::os::windows::io::{AsRawHandle, BorrowedHandle};
        // Duplicate the process handle so the job outlives tokio's handle.
        // SAFETY: The child was just spawned, so its raw handle is valid.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
        let owned = borrowed.try_clone_to_owned()?;
        super::win_job::assign_to_kill_on_close_job(owned.as_raw_handle())?
    };

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
        Ok(ChildOutcome {
            exit_status,
            #[cfg(fspy)]
            path_accesses: None,
        })
    }
    .boxed_local();

    Ok(wait)
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

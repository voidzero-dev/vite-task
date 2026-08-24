//! FreeBSD backend for `fspy`.
//!
//! Linux uses a seccomp-user-notifications supervisor and macOS uses Detours
//! + `LD_PRELOAD` interposition to record the file-system accesses of a child
//! process. FreeBSD has neither of those facilities in this crate, so it
//! provides a no-op backend: the child process is spawned and waited on
//! normally, and [`PathAccessIterable::iter`] returns an empty iterator.
//!
//! Callers (e.g. `vp_command::run_command_with_fspy`) keep working on
//! FreeBSD — they simply observe zero path accesses, which is an honest
//! result for a platform without tracing support.

use std::io;

use futures_util::future::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::{ChildTermination, Command, TrackedChild, error::SpawnError};
use fspy_shared::ipc::PathAccess;

/// No-op backend: initialize without creating any on-disk artifacts.
pub struct SpyImpl;

impl SpyImpl {
    /// Initialize the (empty) backend. `dir` is unused on FreeBSD because
    /// there is no preload library or Detours artifact to materialize.
    #[allow(clippy::unused_self, reason = "init_in takes a directory for parity with the other backends")]
    pub fn init_in(_dir: &std::path::Path) -> io::Result<Self> {
        Ok(Self)
    }

    /// Spawn the command and wait for its status; report no path accesses.
    pub async fn spawn(
        &self,
        command: Command,
        cancellation_token: CancellationToken,
    ) -> Result<TrackedChild, SpawnError> {
        // `into_tokio_command` applies `current_dir`, `arg0`, args, envs,
        // stdio, and any pre_exec closures the caller registered. On FreeBSD
        // none of the supervision machinery is attached, so this is a plain
        // spawn. `tokio::process::Command::spawn` is synchronous (and the
        // pre_exec closures may block), so run it off the async runtime the
        // same way the Linux/macOS backends do.
        let mut tokio_command = command.into_tokio_command();

        let mut child = tokio::task::spawn_blocking(move || tokio_command.spawn())
            .await
            .map_err(|err| SpawnError::OsSpawn(err.into()))?
            .map_err(SpawnError::OsSpawn)?;

        // Take the stdio handles before `child` is moved into the background
        // wait task, matching the Linux/macOS backends.
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Keep polling for the child to exit in the background even if the
        // caller never awaits the wait handle; this matches the Linux/macOS
        // backends (which also need to release supervision resources on
        // exit).
        let wait_handle = tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status?,
                () = cancellation_token.cancelled() => {
                    child.start_kill()?;
                    child.wait().await?
                }
            };
            io::Result::Ok(ChildTermination {
                status,
                path_accesses: Ok(PathAccessIterable),
            })
        })
        .map(|f| f?) // flatten JoinError and io::Result
        .boxed();

        Ok(TrackedChild {
            stdin,
            stdout,
            stderr,
            wait_handle,
        })
    }
}

/// No-op path-access iterator: yields no entries on FreeBSD.
pub struct PathAccessIterable;

impl PathAccessIterable {
    pub fn iter(&self) -> impl Iterator<Item = PathAccess<'_>> {
        std::iter::empty()
    }
}

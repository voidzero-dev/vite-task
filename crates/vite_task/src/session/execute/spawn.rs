//! Process spawning with file system tracking via fspy.

use std::{
    collections::hash_map::Entry,
    io::Write,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use bincode::{Decode, Encode};
use fspy::AccessMode;
use rustc_hash::FxHashSet;
use serde::Serialize;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::SpawnCommand;
use wax::Program as _;

use crate::collections::HashMap;

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Output kind for stdout/stderr
#[derive(Debug, PartialEq, Eq, Clone, Copy, Encode, Decode, Serialize)]
pub enum OutputKind {
    StdOut,
    StdErr,
}

/// Output chunk with stream kind
#[derive(Debug, Encode, Decode, Serialize, Clone)]
pub struct StdOutput {
    pub kind: OutputKind,
    pub content: Vec<u8>,
}

/// Result of spawning a process with file tracking
#[derive(Debug)]
pub struct SpawnResult {
    pub exit_status: ExitStatus,
    pub duration: Duration,
}

/// Tracked file accesses from fspy.
/// Only populated when fspy tracking is enabled (`includes_auto` is true).
#[derive(Default, Debug)]
pub struct TrackedPathAccesses {
    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: FxHashSet<RelativePathBuf>,
}

#[expect(
    clippy::disallowed_types,
    reason = "fspy strip_path_prefix exposes std::path::Path; convert to RelativePathBuf immediately"
)]
fn normalize_tracked_workspace_path(
    stripped_path: &std::path::Path,
    resolved_negatives: &[wax::Glob<'static>],
) -> Option<RelativePathBuf> {
    // On Windows, paths are possible to be still absolute after stripping the workspace root.
    // For example: c:\workspace\subdir\c:\workspace\subdir
    // Just ignore those accesses.
    let relative = RelativePathBuf::new(stripped_path).ok()?;

    // Clean `..` components — fspy may report paths like
    // `packages/sub-pkg/../shared/dist/output.js`. Normalize them for
    // consistent behavior across platforms and clean user-facing messages.
    let relative = relative.clean().ok()?;

    // Skip .git directory accesses (workaround for tools like oxlint)
    if relative.as_path().strip_prefix(".git").is_ok() {
        return None;
    }

    if !resolved_negatives.is_empty()
        && resolved_negatives.iter().any(|neg| neg.is_match(relative.as_str()))
    {
        return None;
    }

    Some(relative)
}

/// How the child process is awaited after stdout/stderr are drained.
enum ChildWait {
    /// fspy tracking enabled — fspy manages cancellation internally.
    Fspy(fspy::TrackedChild),

    /// Plain tokio process — cancellation is handled in the pipe read loop.
    Tokio(tokio::process::Child),
}

/// Spawn a command with optional file system tracking via fspy, using piped stdio.
///
/// Returns the execution result including exit status and duration.
///
/// - stdin is always `/dev/null` (piped mode is for non-interactive execution).
/// - `stdout_writer`/`stderr_writer` receive the child's stdout/stderr output in real-time.
/// - `std_outputs` if provided, will be populated with captured outputs for cache replay.
/// - `path_accesses` if provided, fspy will be used to track file accesses. If `None`, fspy is disabled.
/// - `resolved_negatives` - resolved negative glob patterns for filtering fspy-tracked paths.
#[tracing::instrument(level = "debug", skip_all)]
#[expect(
    clippy::too_many_lines,
    reason = "spawn logic is inherently sequential and splitting would reduce clarity"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "spawn parameters are all distinct concerns that don't form a natural group"
)]
pub async fn spawn_with_tracking(
    spawn_command: &SpawnCommand,
    workspace_root: &AbsolutePath,
    stdout_writer: &mut dyn Write,
    stderr_writer: &mut dyn Write,
    std_outputs: Option<&mut Vec<StdOutput>>,
    path_accesses: Option<&mut TrackedPathAccesses>,
    resolved_negatives: &[wax::Glob<'static>],
    fast_fail_token: CancellationToken,
) -> anyhow::Result<SpawnResult> {
    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(vite_str::Str::as_str));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // On Windows, assign the child to a Job Object so that killing the child also
    // kills all descendant processes (e.g., node.exe spawned by a .cmd shim).
    #[cfg(windows)]
    let job;

    let (mut child_stdout, mut child_stderr, mut child_wait) = if path_accesses.is_some() {
        // fspy tracking enabled — fspy manages cancellation internally via a clone
        // of the token. We keep the original for the pipe read loop.
        let mut tracked_child = cmd.spawn(fast_fail_token.clone()).await?;
        let stdout = tracked_child.stdout.take().unwrap();
        let stderr = tracked_child.stderr.take().unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            job = super::win_job::assign_to_kill_on_close_job(
                tracked_child.process_handle.as_raw_handle(),
            )?;
        }
        (stdout, stderr, ChildWait::Fspy(tracked_child))
    } else {
        let mut child = cmd.into_tokio_command().spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::io::{AsRawHandle, BorrowedHandle};
            // SAFETY: The child was just spawned, so its raw handle is valid.
            let borrowed = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) };
            let owned = borrowed.try_clone_to_owned()?;
            job = super::win_job::assign_to_kill_on_close_job(owned.as_raw_handle())?;
        }
        (stdout, stderr, ChildWait::Tokio(child))
    };

    // Output capturing is independent of fspy tracking
    let mut outputs = std_outputs;
    let mut stdout_buf = [0u8; 8192];
    let mut stderr_buf = [0u8; 8192];
    let mut stdout_done = false;
    let mut stderr_done = false;

    let start = Instant::now();

    // Read from both stdout and stderr concurrently using select!
    // Cancellation is handled directly in the loop: kill the child process (and
    // on Windows, terminate the Job Object to kill grandchildren holding pipes).
    loop {
        if stdout_done && stderr_done {
            break;
        }
        tokio::select! {
            result = child_stdout.read(&mut stdout_buf), if !stdout_done => {
                match result? {
                    0 => stdout_done = true,
                    n => {
                        let content = stdout_buf[..n].to_vec();
                        // Write to the sync writer immediately
                        stdout_writer.write_all(&content)?;
                        stdout_writer.flush()?;
                        // Store outputs for caching
                        if let Some(outputs) = &mut outputs {
                            if let Some(last) = outputs.last_mut()
                                && last.kind == OutputKind::StdOut
                            {
                                last.content.extend(&content);
                            } else {
                                outputs.push(StdOutput { kind: OutputKind::StdOut, content });
                            }
                        }
                    }
                }
            }
            result = child_stderr.read(&mut stderr_buf), if !stderr_done => {
                match result? {
                    0 => stderr_done = true,
                    n => {
                        let content = stderr_buf[..n].to_vec();
                        // Write to the sync writer immediately
                        stderr_writer.write_all(&content)?;
                        stderr_writer.flush()?;
                        // Store outputs for caching
                        if let Some(outputs) = &mut outputs {
                            if let Some(last) = outputs.last_mut()
                                && last.kind == OutputKind::StdErr
                            {
                                last.content.extend(&content);
                            } else {
                                outputs.push(StdOutput { kind: OutputKind::StdErr, content });
                            }
                        }
                    }
                }
            }
            () = fast_fail_token.cancelled() => {
                // Kill the direct child (no-op for fspy which handles it internally).
                if let ChildWait::Tokio(ref mut child) = child_wait {
                    let _ = child.start_kill();
                }
                // On Windows, terminate the entire process tree so grandchild
                // processes release their pipe handles.
                #[cfg(windows)]
                job.terminate();
                break;
            }
        }
    }

    // Wait for process termination and collect results.
    // The child may have closed its pipes without exiting (e.g., daemonized),
    // so we still need a cancellation arm here.
    match child_wait {
        ChildWait::Fspy(tracked_child) => {
            // fspy's wait_handle already monitors the cancellation token internally,
            // so no additional select! is needed here.
            let termination = tracked_child.wait_handle.await?;
            let duration = start.elapsed();

            // path_accesses must be Some when fspy is enabled (they're set together)
            let path_accesses = path_accesses.ok_or_else(|| {
                anyhow::anyhow!("internal error: fspy enabled but path_accesses is None")
            })?;
            let path_reads = &mut path_accesses.path_reads;
            let path_writes = &mut path_accesses.path_writes;

            for access in termination.path_accesses.iter() {
                // Strip workspace root, clean `..` components, and filter in one pass.
                // fspy may report paths like `packages/sub-pkg/../shared/dist/output.js`.
                let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
                    let Ok(stripped_path) = strip_result else {
                        return None;
                    };
                    normalize_tracked_workspace_path(stripped_path, resolved_negatives)
                });

                let Some(relative_path) = relative_path else {
                    continue;
                };

                if access.mode.contains(AccessMode::READ) {
                    path_reads
                        .entry(relative_path.clone())
                        .or_insert(PathRead { read_dir_entries: false });
                }
                if access.mode.contains(AccessMode::WRITE) {
                    path_writes.insert(relative_path.clone());
                }
                if access.mode.contains(AccessMode::READ_DIR) {
                    match path_reads.entry(relative_path) {
                        Entry::Occupied(mut occupied) => {
                            occupied.get_mut().read_dir_entries = true;
                        }
                        Entry::Vacant(vacant) => {
                            vacant.insert(PathRead { read_dir_entries: true });
                        }
                    }
                }
            }

            tracing::debug!(
                "spawn finished, path_reads: {}, path_writes: {}, exit_status: {}",
                path_reads.len(),
                path_writes.len(),
                termination.status,
            );

            Ok(SpawnResult { exit_status: termination.status, duration })
        }
        ChildWait::Tokio(mut child) => {
            let exit_status = tokio::select! {
                status = child.wait() => status?,
                () = fast_fail_token.cancelled() => {
                    child.start_kill()?;
                    child.wait().await?
                }
            };
            Ok(SpawnResult { exit_status, duration: start.elapsed() })
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn malformed_windows_drive_path_after_workspace_strip_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let relative_path =
            normalize_tracked_workspace_path(std::path::Path::new(r"foo\C:\bar"), &[]);
        assert!(relative_path.is_none());
    }
}

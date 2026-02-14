//! Process spawning with file system tracking via fspy.

use std::{
    collections::hash_map::Entry,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use bincode::{Decode, Encode};
use bstr::BString;
use fspy::AccessMode;
use rustc_hash::FxHashSet;
use serde::Serialize;
use tokio::io::AsyncReadExt as _;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::SpawnCommand;

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

/// Tracking result from a spawned process for caching
#[derive(Default, Debug)]
pub struct SpawnTrackResult {
    /// captured stdout/stderr
    pub std_outputs: Vec<StdOutput>,

    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: FxHashSet<RelativePathBuf>,
}

/// Spawn a command with file system tracking via fspy.
///
/// Returns the execution result including captured outputs, exit status,
/// and tracked file accesses.
///
/// - `stdin` controls the child process's stdin (typically `Stdio::null()` or `Stdio::inherit()`).
/// - `on_output` is called in real-time as stdout/stderr data arrives.
/// - `track_result` if provided, will be populated with captured outputs and path accesses for caching. If `None`, tracking is disabled.
#[expect(
    clippy::too_many_lines,
    reason = "spawn logic is inherently sequential and splitting would reduce clarity"
)]
pub async fn spawn_with_tracking<F>(
    spawn_command: &SpawnCommand,
    workspace_root: &AbsolutePath,
    stdin: Stdio,
    mut on_output: F,
    track_result: Option<&mut SpawnTrackResult>,
) -> anyhow::Result<SpawnResult>
where
    F: FnMut(OutputKind, BString),
{
    /// The tracking state of the spawned process
    enum TrackingState<'a> {
        /// Tacking is enabled, with the tracked child and result reference
        Enabled(fspy::TrackedChild, &'a mut SpawnTrackResult),

        /// Tracking is disabled, with the tokio child process
        Disabled(tokio::process::Child),
    }

    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(vite_str::Str::as_str));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut tracking_state = if let Some(track_result) = track_result {
        // track_result is Some. Spawn with tracking enabled
        TrackingState::Enabled(cmd.spawn().await?, track_result)
    } else {
        // Spawn without tracking
        TrackingState::Disabled(cmd.into_tokio_command().spawn()?)
    };

    let mut child_stdout = match &mut tracking_state {
        TrackingState::Enabled(tracked_child, _) => tracked_child.stdout.take().unwrap(),
        TrackingState::Disabled(tokio_child) => tokio_child.stdout.take().unwrap(),
    };
    let mut child_stderr = match &mut tracking_state {
        TrackingState::Enabled(tracked_child, _) => tracked_child.stderr.take().unwrap(),
        TrackingState::Disabled(tokio_child) => tokio_child.stderr.take().unwrap(),
    };

    let mut outputs = match &mut tracking_state {
        TrackingState::Enabled(_, track_result) => Some(&mut track_result.std_outputs),
        TrackingState::Disabled(_) => None,
    };
    let mut stdout_buf = [0u8; 8192];
    let mut stderr_buf = [0u8; 8192];
    let mut stdout_done = false;
    let mut stderr_done = false;

    let start = Instant::now();

    // Helper closure to process output chunks
    let mut process_output = |kind: OutputKind, content: Vec<u8>| {
        // Emit event immediately
        on_output(kind, content.clone().into());

        // Store outputs for caching
        if let Some(outputs) = &mut outputs {
            // Merge consecutive outputs of the same kind for caching
            if let Some(last) = outputs.last_mut()
                && last.kind == kind
            {
                last.content.extend(&content);
            } else {
                outputs.push(StdOutput { kind, content });
            }
        }
    };

    // Read from both stdout and stderr concurrently using select!
    loop {
        tokio::select! {
            result = child_stdout.read(&mut stdout_buf), if !stdout_done => {
                match result? {
                    0 => stdout_done = true,
                    n => process_output(OutputKind::StdOut, stdout_buf[..n].to_vec()),
                }
            }
            result = child_stderr.read(&mut stderr_buf), if !stderr_done => {
                match result? {
                    0 => stderr_done = true,
                    n => process_output(OutputKind::StdErr, stderr_buf[..n].to_vec()),
                }
            }
            else => break,
        }
    }

    let (termination, track_result) = match tracking_state {
        TrackingState::Enabled(tracked_child, track_result) => {
            (tracked_child.wait_handle.await?, track_result)
        }
        TrackingState::Disabled(mut tokio_child) => {
            return Ok(SpawnResult {
                exit_status: tokio_child.wait().await?,
                duration: start.elapsed(),
            });
        }
    };
    let duration = start.elapsed();

    // Process path accesses
    let path_reads = &mut track_result.path_reads;
    let path_writes = &mut track_result.path_writes;

    for access in termination.path_accesses.iter() {
        let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
            let Ok(stripped_path) = strip_result else {
                return None;
            };
            // On Windows, paths are possible to be still absolute after stripping the workspace root.
            // For example: c:\workspace\subdir\c:\workspace\subdir
            // Just ignore those accesses.
            RelativePathBuf::new(stripped_path).ok()
        });

        let Some(relative_path) = relative_path else {
            // Ignore accesses outside the workspace
            continue;
        };

        // Skip .git directory accesses (workaround for tools like oxlint)
        if relative_path.as_path().strip_prefix(".git").is_ok() {
            continue;
        }

        if access.mode.contains(AccessMode::READ) {
            path_reads.entry(relative_path.clone()).or_insert(PathRead { read_dir_entries: false });
        }
        if access.mode.contains(AccessMode::WRITE) {
            path_writes.insert(relative_path.clone());
        }
        if access.mode.contains(AccessMode::READ_DIR) {
            match path_reads.entry(relative_path) {
                Entry::Occupied(mut occupied) => occupied.get_mut().read_dir_entries = true,
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

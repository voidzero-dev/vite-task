//! Process spawning with file system tracking via fspy.

use std::{
    collections::hash_map::Entry,
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};

use bincode::{Decode, Encode};
use bstr::BString;
use fspy::AccessMode;
use serde::Serialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::SpawnCommand;

use crate::collections::HashMap;

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Path write access info
#[derive(Debug, Clone, Copy)]
pub struct PathWrite;

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
    /// Whether stdin had data forwarded to the child
    pub stdin_had_data: bool,
}

/// Tracking result from a spawned process for caching
#[derive(Default, Debug)]
pub struct SpawnTrackResult {
    /// captured stdout/stderr
    pub std_outputs: Vec<StdOutput>,

    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: HashMap<RelativePathBuf, PathWrite>,
}

/// Spawn a command with file system tracking via fspy.
///
/// Returns the execution result including captured outputs, exit status,
/// and tracked file accesses.
///
/// # Arguments
/// - `spawn_command`: The command to spawn with its arguments, environment, and working directory.
/// - `workspace_root`: Base path for converting absolute paths to relative paths in tracking.
/// - `on_output`: Callback invoked in real-time as stdout/stderr data arrives.
/// - `track_result`: If provided, will be populated with captured outputs and path accesses
///   for caching. If `None`, tracking is disabled and the command runs without fspy overhead.
///
/// # Concurrent I/O Architecture
///
/// This function manages three concurrent I/O operations:
/// 1. **drain_outputs**: Reads stdout and stderr until both reach EOF
/// 2. **forward_stdin**: Forwards parent's stdin to child until EOF
/// 3. **wait_for_exit**: Waits for child process to terminate
///
/// Each operation is a separate future with its own internal loop. This design avoids
/// a single large `select!` loop with many condition flags, making the code easier to
/// understand and maintain.
///
/// # Deadlock Avoidance
///
/// All three operations run concurrently. This is critical for commands that depend on
/// stdin, like `node -e "process.stdin.pipe(process.stdout)"`. If we waited for stdout
/// to close before forwarding stdin, we would deadlock because stdout won't close until
/// stdin closes.
///
/// # Cancellation
///
/// When the child exits, `forward_stdin` is implicitly cancelled by breaking out of
/// the coordination loop. This is safe because stdin data after child exit is meaningless.
pub async fn spawn_with_tracking<F>(
    spawn_command: &SpawnCommand,
    workspace_root: &AbsolutePath,
    mut on_output: F,
    track_result: Option<&mut SpawnTrackResult>,
) -> anyhow::Result<SpawnResult>
where
    F: FnMut(OutputKind, BString),
{
    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(|arg| arg.as_str()));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // Spawn with or without tracking based on track_result.
    //
    // WaitHandle is separated from track_result to avoid borrow checker issues:
    // - track_result needs to be borrowed mutably after the spawn
    // - wait_handle needs to be moved into a future
    // By keeping them separate, we can move wait_handle while retaining track_result.
    enum WaitHandle {
        /// Tracked spawn via fspy - returns termination info with file access data
        Tracked(futures_util::future::BoxFuture<'static, std::io::Result<fspy::ChildTermination>>),
        /// Untracked spawn via tokio - just returns exit status
        Untracked(tokio::process::Child),
    }

    let (
        mut child_stdout,
        mut child_stderr,
        child_stdin,
        mut wait_handle,
        track_result,
        track_enabled,
    ) = if let Some(track_result) = track_result {
        let mut tracked = cmd.spawn().await?;
        let stdout = tracked.stdout.take().unwrap();
        let stderr = tracked.stderr.take().unwrap();
        let stdin = tracked.stdin.take();
        (stdout, stderr, stdin, WaitHandle::Tracked(tracked.wait_handle), Some(track_result), true)
    } else {
        let mut child = cmd.into_tokio_command().spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdin = child.stdin.take();
        (stdout, stderr, stdin, WaitHandle::Untracked(child), None, false)
    };

    // Local buffer for captured outputs. This is separate from track_result to allow
    // the drain_outputs future to own a mutable reference without conflicting with
    // track_result's lifetime.
    let mut std_outputs: Vec<StdOutput> = Vec::new();
    let start = Instant::now();

    // Future that drains both stdout and stderr until EOF.
    //
    // Uses an internal select! loop to read from whichever stream has data available.
    // Consecutive outputs of the same kind are merged into a single StdOutput entry
    // to reduce storage overhead when caching.
    let drain_outputs = async {
        let mut stdout_buf = [0u8; 8192];
        let mut stderr_buf = [0u8; 8192];
        let mut stdout_done = false;
        let mut stderr_done = false;

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = child_stdout.read(&mut stdout_buf), if !stdout_done => {
                    match result? {
                        0 => stdout_done = true,
                        n => {
                            let content = stdout_buf[..n].to_vec();
                            on_output(OutputKind::StdOut, content.clone().into());
                            if track_enabled {
                                if let Some(last) = std_outputs.last_mut()
                                    && last.kind == OutputKind::StdOut
                                {
                                    last.content.extend(&content);
                                } else {
                                    std_outputs.push(StdOutput { kind: OutputKind::StdOut, content });
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
                            on_output(OutputKind::StdErr, content.clone().into());
                            if track_enabled {
                                if let Some(last) = std_outputs.last_mut()
                                    && last.kind == OutputKind::StdErr
                                {
                                    last.content.extend(&content);
                                } else {
                                    std_outputs.push(StdOutput { kind: OutputKind::StdErr, content });
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok::<_, std::io::Error>(())
    };

    // Future that forwards stdin from parent to child until EOF.
    //
    // This runs concurrently with drain_outputs to avoid deadlock: some commands
    // (like `cat` or `node -e "process.stdin.pipe(process.stdout)"`) won't produce
    // output until they receive input, so we must forward stdin while draining outputs.
    //
    // When parent stdin reaches EOF, we drop the child stdin handle to signal EOF
    // to the child process.
    //
    // Returns whether any data was forwarded - this is used to disable caching since
    // the output may depend on the input data which we don't fingerprint.
    let forward_stdin = async {
        let mut buf = [0u8; 8192];
        let mut stdin_had_data = false;
        let mut parent_stdin = tokio::io::stdin();
        let mut child_stdin = child_stdin;

        loop {
            match parent_stdin.read(&mut buf).await? {
                0 => {
                    // EOF on parent stdin - close child stdin to signal EOF
                    drop(child_stdin.take());
                    break Ok::<_, std::io::Error>(stdin_had_data);
                }
                n => {
                    stdin_had_data = true;
                    if let Some(ref mut stdin) = child_stdin {
                        stdin.write_all(&buf[..n]).await?;
                    }
                }
            }
        }
    };

    // Future that waits for child to exit.
    // Returns Some(ChildTermination) for tracked spawns, None for untracked.
    let wait_for_exit = async {
        match &mut wait_handle {
            WaitHandle::Tracked(wait) => wait.await.map(Some),
            WaitHandle::Untracked(child) => child.wait().await.map(|_| None),
        }
    };

    // Pin all futures for use in the coordination loop.
    // We can't use join!/select! macros directly because:
    // 1. drain_outputs must complete before wait_for_exit (child won't exit until we drain pipes)
    // 2. forward_stdin should be cancelled when child exits (not waited on)
    // 3. We need to track intermediate state (stdin_result)
    futures_util::pin_mut!(drain_outputs, forward_stdin, wait_for_exit);

    // State flags for the coordination loop
    let mut drain_done = false;
    let mut stdin_result: Option<Result<bool, std::io::Error>> = None;
    let termination: Option<fspy::ChildTermination>;

    // Coordination loop: orchestrates the three concurrent operations.
    //
    // The select! conditions ensure correct ordering:
    // - drain_outputs runs unconditionally until done
    // - forward_stdin runs until it completes or child exits (whichever first)
    // - wait_for_exit only runs after drain_outputs is done (we must drain pipes first)
    //
    // When wait_for_exit completes, we break out of the loop. This implicitly cancels
    // any pending stdin read, which is safe and intentional.
    loop {
        tokio::select! {
            // Drain stdout/stderr - must complete before we can wait for exit
            result = &mut drain_outputs, if !drain_done => {
                result?;
                drain_done = true;
            }
            // Forward stdin - record result but don't block child exit
            result = &mut forward_stdin, if stdin_result.is_none() => {
                stdin_result = Some(result);
            }
            // Wait for child exit - only after drain is done
            result = &mut wait_for_exit, if drain_done => {
                termination = result?;
                break;
            }
        }
    }

    let duration = start.elapsed();

    // Extract stdin_had_data from the result. If stdin read was cancelled (None) or
    // errored, we conservatively assume no data was forwarded.
    let stdin_had_data = stdin_result.map(|r| r.unwrap_or(false)).unwrap_or(false);

    // Get exit status from termination info
    let exit_status = match &termination {
        Some(term) => term.status,
        None => ExitStatus::default(), // Untracked path - status already consumed
    };

    // If tracking was disabled, return early without processing file accesses
    let Some(track_result) = track_result else {
        return Ok(SpawnResult { exit_status, duration, stdin_had_data });
    };

    // Copy captured outputs from local buffer to track_result for caching
    track_result.std_outputs = std_outputs;

    // Process path accesses from fspy tracking.
    // These are used to build the post-run fingerprint for cache invalidation.
    let termination = termination.expect("termination should be Some when tracking is enabled");
    let path_reads = &mut track_result.path_reads;
    let path_writes = &mut track_result.path_writes;

    for access in termination.path_accesses.iter() {
        // Convert absolute paths to workspace-relative paths.
        // Paths outside the workspace are ignored (e.g., system libraries).
        let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
            let Ok(stripped_path) = strip_result else {
                return None;
            };
            RelativePathBuf::new(stripped_path).ok()
        });

        let Some(relative_path) = relative_path else {
            continue;
        };

        // Skip .git directory - these are internal git operations that shouldn't
        // affect cache fingerprinting (e.g., reading HEAD, refs).
        if relative_path.as_path().strip_prefix(".git").is_ok() {
            continue;
        }

        // Track read accesses for fingerprinting input files
        if access.mode.contains(AccessMode::READ) {
            path_reads.entry(relative_path.clone()).or_insert(PathRead { read_dir_entries: false });
        }
        // Track write accesses (for future use - output fingerprinting)
        if access.mode.contains(AccessMode::WRITE) {
            path_writes.insert(relative_path.clone(), PathWrite);
        }
        // Track directory reads (e.g., readdir) which may affect cache validity
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
        exit_status,
    );

    Ok(SpawnResult { exit_status, duration, stdin_had_data })
}

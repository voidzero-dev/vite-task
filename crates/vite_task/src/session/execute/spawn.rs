//! Process spawning with file system tracking via fspy.

use std::{
    collections::hash_map::Entry,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bincode::{Decode, Encode};
use fspy::AccessMode;
use futures_util::future::try_join3;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_task_plan::SpawnCommand;

use crate::{Error, collections::HashMap};

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
    pub std_outputs: Arc<[StdOutput]>,
    pub exit_status: ExitStatus,
    pub path_reads: HashMap<RelativePathBuf, PathRead>,
    pub path_writes: HashMap<RelativePathBuf, PathWrite>,
    pub duration: Duration,
}

/// Collects stdout/stderr into `outputs` and simultaneously writes to real stdout/stderr
async fn collect_std_outputs(
    outputs: &Mutex<Vec<StdOutput>>,
    mut stream: impl AsyncRead + Unpin,
    kind: OutputKind,
) -> Result<(), Error> {
    let mut buf = [0u8; 8192];
    let mut parent_output_handle: Box<dyn AsyncWrite + Unpin + Send> = match kind {
        OutputKind::StdOut => Box::new(tokio::io::stdout()),
        OutputKind::StdErr => Box::new(tokio::io::stderr()),
    };
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        let content = &buf[..n];
        parent_output_handle.write_all(content).await?;
        parent_output_handle.flush().await?;

        // Merge consecutive outputs of the same kind
        let mut outputs = outputs.lock().unwrap();
        if let Some(last) = outputs.last_mut()
            && last.kind == kind
        {
            last.content.extend_from_slice(content);
        } else {
            outputs.push(StdOutput { kind, content: content.to_vec() });
        }
    }
}

/// Spawn a command with file system tracking via fspy.
///
/// Returns the execution result including captured outputs, exit status,
/// and tracked file accesses.
pub async fn spawn_with_tracking(
    spawn_command: &SpawnCommand,
    workspace_root: &AbsolutePath,
) -> Result<SpawnResult, Error> {
    let mut cmd = fspy::Command::new(spawn_command.program_path.as_path());
    cmd.args(spawn_command.args.iter().map(|arg| arg.as_str()));
    cmd.envs(spawn_command.all_envs.iter());
    cmd.current_dir(&*spawn_command.cwd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().await?;

    let child_stdout = child.stdout.take().unwrap();
    let child_stderr = child.stderr.take().unwrap();

    let outputs = Mutex::new(Vec::<StdOutput>::new());

    let ((), (), (termination, duration)) = try_join3(
        collect_std_outputs(&outputs, child_stdout, OutputKind::StdOut),
        collect_std_outputs(&outputs, child_stderr, OutputKind::StdErr),
        async {
            let start = Instant::now();
            let exit_status = child.wait_handle.await?;
            Ok((exit_status, start.elapsed()))
        },
    )
    .await?;

    // Process path accesses
    let mut path_reads = HashMap::<RelativePathBuf, PathRead>::new();
    let mut path_writes = HashMap::<RelativePathBuf, PathWrite>::new();

    for access in termination.path_accesses.iter() {
        let relative_path = access
            .path
            .strip_path_prefix(workspace_root, |strip_result| {
                let Ok(stripped_path) = strip_result else {
                    return None;
                };
                Some(RelativePathBuf::new(stripped_path).map_err(|err| {
                    Error::InvalidRelativePath { path: stripped_path.into(), reason: err }
                }))
            })
            .transpose()?;

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
            path_writes.insert(relative_path.clone(), PathWrite);
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

    let outputs = outputs.into_inner().unwrap();
    tracing::debug!(
        "spawn finished, path_reads: {}, path_writes: {}, outputs: {}, exit_status: {}",
        path_reads.len(),
        path_writes.len(),
        outputs.len(),
        termination.status,
    );

    Ok(SpawnResult {
        std_outputs: outputs.into(),
        exit_status: termination.status,
        path_reads,
        path_writes,
        duration,
    })
}

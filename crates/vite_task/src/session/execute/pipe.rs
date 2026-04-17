//! Drain child stdout/stderr concurrently to writers, with optional capture.

use std::io::Write;

use serde::Serialize;
use tokio::{
    io::AsyncReadExt as _,
    process::{ChildStderr, ChildStdout},
};
use tokio_util::sync::CancellationToken;
use wincode::{SchemaRead, SchemaWrite};

/// Output kind for stdout/stderr
#[derive(Debug, PartialEq, Eq, Clone, Copy, SchemaWrite, SchemaRead, Serialize)]
pub enum OutputKind {
    StdOut,
    StdErr,
}

/// Output chunk with stream kind
#[derive(Debug, SchemaWrite, SchemaRead, Serialize, Clone)]
pub struct StdOutput {
    pub kind: OutputKind,
    pub content: Vec<u8>,
}

/// Downstream destinations for bytes read from the child's stdout/stderr:
/// two pass-through writers plus an optional capture buffer (populated in
/// place during drain for cache replay).
pub struct PipeSinks<'a> {
    pub stdout_writer: &'a mut dyn Write,
    pub stderr_writer: &'a mut dyn Write,
    pub capture: Option<&'a mut Vec<StdOutput>>,
}

/// Drain the child's stdout/stderr concurrently into `sinks`.
///
/// Bytes are written through `sinks.stdout_writer` / `sinks.stderr_writer` in
/// real time and, when `sinks.capture` is `Some`, also appended (with adjacent
/// same-kind chunks coalesced) for cache replay.
///
/// On cancellation: returns `Ok(())` without killing the child — the caller
/// drives the child's cancellation-aware `wait` future next, which observes the
/// same already-fired token and performs the kill. Dropping `stdout`/`stderr`
/// closes the pipe read ends (EPIPE on Unix, `ERROR_BROKEN_PIPE` on Windows).
#[tracing::instrument(level = "debug", skip_all)]
pub async fn pipe_stdio(
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    mut sinks: PipeSinks<'_>,
    cancellation_token: CancellationToken,
) -> std::io::Result<()> {
    let mut stdout_buf = [0u8; 8192];
    let mut stderr_buf = [0u8; 8192];
    let mut stdout_done = false;
    let mut stderr_done = false;

    loop {
        if stdout_done && stderr_done {
            return Ok(());
        }
        tokio::select! {
            result = stdout.read(&mut stdout_buf), if !stdout_done => {
                match result? {
                    0 => stdout_done = true,
                    n => {
                        let bytes = &stdout_buf[..n];
                        sinks.stdout_writer.write_all(bytes)?;
                        sinks.stdout_writer.flush()?;
                        if let Some(capture) = &mut sinks.capture {
                            append_output_chunk(capture, OutputKind::StdOut, bytes);
                        }
                    }
                }
            }
            result = stderr.read(&mut stderr_buf), if !stderr_done => {
                match result? {
                    0 => stderr_done = true,
                    n => {
                        let bytes = &stderr_buf[..n];
                        sinks.stderr_writer.write_all(bytes)?;
                        sinks.stderr_writer.flush()?;
                        if let Some(capture) = &mut sinks.capture {
                            append_output_chunk(capture, OutputKind::StdErr, bytes);
                        }
                    }
                }
            }
            () = cancellation_token.cancelled() => {
                return Ok(());
            }
        }
    }
}

fn append_output_chunk(capture: &mut Vec<StdOutput>, kind: OutputKind, bytes: &[u8]) {
    if let Some(last) = capture.last_mut()
        && last.kind == kind
    {
        last.content.extend_from_slice(bytes);
    } else {
        capture.push(StdOutput { kind, content: bytes.to_vec() });
    }
}

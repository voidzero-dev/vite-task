//! Supervisor side of the blocking-callback channel for the Windows backend.
//!
//! A traced process connects to a named pipe and, per open/close event, sends
//! a length-prefixed [`CallbackRequest`] followed by the raw file handle. The
//! supervisor duplicates that handle out of the traced process, runs the user
//! callback while the traced process blocks, then writes back a single
//! [`CALLBACK_ACK`] byte to release it.

use std::{
    io,
    os::windows::io::{AsHandle as _, AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use fspy_shared::callback::{CALLBACK_ACK, CallbackKind, CallbackRequest};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;
use winapi::{
    shared::{minwindef::FALSE, ntdef::HANDLE},
    um::{
        handleapi::DuplicateHandle, processthreadsapi::GetCurrentProcess,
        winnt::DUPLICATE_SAME_ACCESS,
    },
};

use super::{BorrowedFile, FileCallbackFn, FileEvent, FileEventKind, FileEventPath};

static PIPE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A running callback server bound to a uniquely-named pipe.
pub struct WindowsCallbackServer {
    pipe_name: Vec<u8>,
    cancel: CancellationToken,
    accept_task: JoinHandle<()>,
    /// The traced child's process handle, set after the child is spawned and
    /// before it is resumed. Needed to duplicate handles out of it.
    process_handle: Arc<OnceLock<OwnedHandle>>,
}

impl WindowsCallbackServer {
    /// Creates the named pipe and starts accepting connections immediately.
    pub fn new(callback: Arc<FileCallbackFn>) -> io::Result<Self> {
        let pipe_name = format!(
            r"\\.\pipe\fspy-callback-{}-{}",
            std::process::id(),
            PIPE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let first = ServerOptions::new().first_pipe_instance(true).create(&pipe_name)?;
        let process_handle = Arc::new(OnceLock::new());
        let cancel = CancellationToken::new();
        let accept_task = tokio::spawn(accept_loop(
            pipe_name.clone(),
            first,
            callback,
            Arc::clone(&process_handle),
            cancel.clone(),
        ));
        Ok(Self { pipe_name: pipe_name.into_bytes(), cancel, accept_task, process_handle })
    }

    /// The pipe name the traced process must connect to (ASCII bytes).
    pub fn pipe_name_bytes(&self) -> &[u8] {
        &self.pipe_name
    }

    /// Provides the traced child's process handle once it has been spawned.
    pub fn set_process_handle(&self, handle: OwnedHandle) {
        let _ = self.process_handle.set(handle);
    }

    /// Stops accepting and waits for every in-flight callback to finish.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.accept_task.await;
    }
}

async fn accept_loop(
    pipe_name: String,
    first: NamedPipeServer,
    callback: Arc<FileCallbackFn>,
    process_handle: Arc<OnceLock<OwnedHandle>>,
    cancel: CancellationToken,
) {
    let mut server = first;
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            result = server.connect() => {
                if result.is_err() {
                    break;
                }
                let connected = std::mem::replace(
                    &mut server,
                    match ServerOptions::new().create(&pipe_name) {
                        Ok(next) => next,
                        Err(_) => break,
                    },
                );
                conns.spawn(handle_connection(
                    connected,
                    Arc::clone(&callback),
                    Arc::clone(&process_handle),
                ));
            }
        }
    }
    // Drain in-flight connections so every callback finishes before the caller
    // locks the shared-memory channel.
    while conns.join_next().await.is_some() {}
}

async fn handle_connection(
    mut pipe: NamedPipeServer,
    callback: Arc<FileCallbackFn>,
    process_handle: Arc<OnceLock<OwnedHandle>>,
) {
    while handle_one(&mut pipe, &callback, &process_handle).await.is_ok() {}
}

async fn handle_one(
    pipe: &mut NamedPipeServer,
    callback: &Arc<FileCallbackFn>,
    process_handle: &Arc<OnceLock<OwnedHandle>>,
) -> io::Result<()> {
    let len = pipe.read_u32_le().await?;
    let mut body = vec![0u8; len as usize];
    pipe.read_exact(&mut body).await?;
    let raw_handle = pipe.read_u64_le().await?;

    let request = ParsedRequest::decode(&body)?;
    if let Some(source_process) = process_handle.get()
        && let Ok(duplicated) = duplicate_handle(source_process, raw_handle)
    {
        run_callback(Arc::clone(callback), request, duplicated).await;
    }

    pipe.write_all(&[CALLBACK_ACK]).await?;
    Ok(())
}

/// Duplicates `raw_handle` out of the traced process into this process.
fn duplicate_handle(source_process: &OwnedHandle, raw_handle: u64) -> io::Result<OwnedHandle> {
    let mut duplicated: HANDLE = std::ptr::null_mut();
    // SAFETY: `source_process` is a valid process handle with `PROCESS_DUP_HANDLE`
    // access; `raw_handle` is a handle value valid inside that process.
    let ok = unsafe {
        DuplicateHandle(
            source_process.as_handle().as_raw_handle().cast(),
            usize::try_from(raw_handle).unwrap_or(0) as HANDLE,
            GetCurrentProcess(),
            &raw mut duplicated,
            0,
            FALSE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == FALSE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `DuplicateHandle` produced a valid, owned handle in this process.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicated.cast()) })
}

struct ParsedRequest {
    kind: FileEventKind,
    pid: u32,
    mode: fspy_shared::ipc::AccessMode,
    path: Option<PathBuf>,
}

impl ParsedRequest {
    fn decode(body: &[u8]) -> io::Result<Self> {
        let request: CallbackRequest<'_> = wincode::deserialize_exact(body)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        let kind = if request.kind == CallbackKind::CLOSING {
            FileEventKind::Closing
        } else {
            FileEventKind::Opened
        };
        let path = request.path.map(|native| PathBuf::from(native.to_cow_os_str().into_owned()));
        Ok(Self { kind, pid: request.pid, mode: request.mode, path })
    }
}

/// Runs the user callback on a blocking thread, then closes the supervisor's
/// duplicated handle before returning (so it is closed before the ACK).
async fn run_callback(callback: Arc<FileCallbackFn>, request: ParsedRequest, handle: OwnedHandle) {
    let result = tokio::task::spawn_blocking(move || {
        let path = match request.kind {
            FileEventKind::Opened => {
                FileEventPath::Open(request.path.as_deref().unwrap_or_else(|| Path::new("")))
            }
            FileEventKind::Closing => FileEventPath::Close(request.path.as_deref()),
        };
        let event = FileEvent {
            kind: request.kind,
            pid: request.pid,
            mode: request.mode,
            path,
            fd: BorrowedFile::new(handle.as_handle()),
        };
        (*callback)(event);
        // `handle` is dropped here, closing the supervisor's duplicate.
    })
    .await;
    drop(result);
}

//! Supervisor side of the blocking-callback channel for the Unix preload
//! backend.
//!
//! A traced process connects (one connection per thread) to a Unix-domain
//! socket and, per open/close event, sends the file descriptor via
//! `SCM_RIGHTS` followed by a length-prefixed [`CallbackRequest`]. The
//! supervisor runs the user callback while the traced process blocks, then
//! writes back a single [`CALLBACK_ACK`] byte to release it.

use std::{
    io,
    os::fd::{AsFd as _, FromRawFd as _, OwnedFd},
    path::{Path, PathBuf},
    sync::Arc,
};

use fspy_shared::callback::{CALLBACK_ACK, CallbackKind, CallbackRequest};
use passfd::tokio::FdPassingExt as _;
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use super::{BorrowedFile, FileCallbackFn, FileEvent, FileEventKind, FileEventPath};

/// A running callback server bound to a temporary Unix-domain socket.
pub struct UnixCallbackServer {
    socket_path: PathBuf,
    cancel: CancellationToken,
    accept_task: JoinHandle<()>,
}

impl UnixCallbackServer {
    /// Bind the socket and start accepting connections immediately.
    pub fn new(callback: Arc<FileCallbackFn>) -> io::Result<Self> {
        let listener = tempfile::Builder::new()
            .prefix("fspy_callback")
            .make(|path| UnixListener::bind(path))?;
        let socket_path = listener.path().to_path_buf();
        let cancel = CancellationToken::new();
        let accept_task = tokio::spawn(accept_loop(listener, callback, cancel.clone()));
        Ok(Self { socket_path, cancel, accept_task })
    }

    /// Filesystem path of the socket the traced process must connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stop accepting and wait for every in-flight callback to finish.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.accept_task.await;
    }
}

async fn accept_loop(
    listener: NamedTempFile<UnixListener>,
    callback: Arc<FileCallbackFn>,
    cancel: CancellationToken,
) {
    let mut conns: JoinSet<()> = JoinSet::new();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.as_file().accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        conns.spawn(handle_connection(stream, Arc::clone(&callback)));
                    }
                    Err(_) => break,
                }
            }
        }
    }
    // Drain in-flight connections so every callback finishes before the caller
    // locks the shared-memory channel.
    while conns.join_next().await.is_some() {}
}

async fn handle_connection(mut stream: UnixStream, callback: Arc<FileCallbackFn>) {
    // A connection serves one traced thread; loop until the peer closes it.
    while handle_one(&mut stream, &callback).await.is_ok() {}
}

async fn handle_one(stream: &mut UnixStream, callback: &Arc<FileCallbackFn>) -> io::Result<()> {
    let raw_fd = stream.recv_fd().await?;
    // SAFETY: `recv_fd` returns a freshly received, owned file descriptor.
    let owned_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    let len = stream.read_u32_le().await?;
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await?;

    let request = ParsedRequest::decode(&body)?;
    run_callback(Arc::clone(callback), request, owned_fd).await;

    stream.write_all(&[CALLBACK_ACK]).await?;
    Ok(())
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
        let path = request.path.map(|native| PathBuf::from(native.as_os_str()));
        Ok(Self { kind, pid: request.pid, mode: request.mode, path })
    }
}

/// Run the user callback on a blocking thread, then close the supervisor's
/// copy of the fd before returning (so it is closed before the ACK is sent).
async fn run_callback(callback: Arc<FileCallbackFn>, request: ParsedRequest, owned_fd: OwnedFd) {
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
            fd: BorrowedFile::new(owned_fd.as_fd()),
        };
        (*callback)(event);
        // `owned_fd` is dropped here, closing the supervisor's copy.
    })
    .await;
    // A panicking callback must not wedge the traced process: the ACK is still
    // sent by the caller. Swallow the join error.
    drop(result);
}

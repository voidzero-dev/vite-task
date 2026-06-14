use std::{
    cell::RefCell,
    ffi::{OsStr, OsString},
    io,
    sync::Arc,
};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use rustc_hash::FxHashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use vite_task_ipc_shared::{GetEnvResponse, IPC_ENV_NAME, Request};
use wincode::{SchemaWrite, config::DefaultConfig};

pub trait Handler {
    fn disable_cache(&mut self);
    fn get_env(&mut self, name: &OsStr) -> Option<Arc<OsStr>>;
}

/// A protocol-level failure observed while servicing a client.
///
/// The driver retains only the first such error across all clients, then
/// completes gracefully (existing clients drain, new connections are rejected).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read message from the task")]
    ReadFrame(#[source] io::Error),

    #[error("invalid message from the task")]
    InvalidRequest(#[source] wincode::ReadError),

    #[error("failed to send response to the task")]
    WriteResponse(#[source] io::Error),
}

/// A [`Handler`] that records every report and resolves `get_env` against
/// provided envs.
///
/// Call [`Recorder::into_reports`] after the driver future completes to
/// recover the collected [`Reports`].
pub struct Recorder {
    cache_disabled: bool,
    env_records: FxHashMap<Arc<OsStr>, Option<Arc<OsStr>>>,
    /// The envs `get_env` resolves against. The runner supplies these for the
    /// spawned task; the server never re-reads the live process env.
    envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>>,
}

/// The data collected by a [`Recorder`] over the server's lifetime.
#[derive(Debug, Default)]
pub struct Reports {
    pub cache_disabled: bool,
    pub env_records: FxHashMap<Arc<OsStr>, Option<Arc<OsStr>>>,
}

impl Recorder {
    #[must_use]
    pub fn new(envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>>) -> Self {
        Self { cache_disabled: false, env_records: FxHashMap::default(), envs }
    }

    #[must_use]
    pub fn into_reports(self) -> Reports {
        Reports { cache_disabled: self.cache_disabled, env_records: self.env_records }
    }
}

impl Handler for Recorder {
    fn disable_cache(&mut self) {
        self.cache_disabled = true;
    }

    fn get_env(&mut self, name: &OsStr) -> Option<Arc<OsStr>> {
        match self.env_records.entry(name.into()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let value = self.envs.get(name).cloned();
                entry.insert(value.clone());
                value
            }
        }
    }
}

/// Handle to a running IPC server.
///
/// `driver` must be polled to accept clients and handle messages. It resolves
/// only after [`StopAccepting::signal`] has been called AND all in-flight
/// per-client tasks have drained, returning the owned handler.
///
/// The driver resolves to `Err(Error)` if any client triggered a protocol
/// violation (see [`Error`]). The first such error is retained; subsequent
/// errors during drain are discarded. On `Err`, the handler is not returned.
///
/// Dropping `driver` before it resolves tears everything down immediately —
/// listener closed, per-client tasks cancelled, handler discarded.
pub struct ServerHandle<'h, H> {
    pub driver: LocalBoxFuture<'h, Result<H, Error>>,
    pub stop_accepting: StopAccepting,
}

/// Signal that tells the server to stop accepting new clients. Existing
/// clients continue until they naturally close the connection; the driver
/// future resolves once that drain completes.
///
/// [`signal`](Self::signal) takes `&self` and the underlying cancellation
/// is idempotent, so calling it twice or from a shared borrow is safe.
pub struct StopAccepting {
    token: CancellationToken,
}

impl StopAccepting {
    /// A no-op `StopAccepting` not bound to any running server. Signalling it
    /// is a no-op. Useful for placeholder paths where the runner hasn't wired
    /// the server in yet but still needs a value of this type.
    #[must_use]
    pub fn noop() -> Self {
        Self { token: CancellationToken::new() }
    }

    pub fn signal(&self) {
        self.token.cancel();
    }
}

/// Starts an IPC server.
///
/// Returns the env entries that a child process must inherit to find and
/// connect to this server, plus a handle bundling the driver future and the
/// `StopAccepting` signal. See [`ServerHandle`] for driver semantics.
///
/// # Errors
///
/// Returns an error if creating the listener fails (on Unix, this includes
/// creating the temp socket path).
pub fn serve<'h, H: Handler + 'h>(
    handler: H,
) -> io::Result<(impl Iterator<Item = (&'static OsStr, OsString)>, ServerHandle<'h, H>)> {
    let stop_token = CancellationToken::new();
    let (name, bound) = bind_listener()?;

    let run_stop = stop_token.clone();
    let driver = async move {
        // Multiple per-client futures coexist inside `FuturesUnordered` and each
        // calls `&mut self` handler methods. `RefCell` provides the interior
        // mutability that makes these shared-access method calls compile; at
        // runtime the `borrow_mut()` never conflicts because we're on a
        // single-threaded runtime and handler methods are synchronous (no
        // awaits, so no borrow spans a yield point).
        let handler = RefCell::new(handler);
        let first_err = run(bound, &handler, run_stop).await;
        first_err.map_or_else(|| Ok(handler.into_inner()), Err)
    }
    .boxed_local();

    Ok((
        std::iter::once((OsStr::new(IPC_ENV_NAME), name)),
        ServerHandle { driver, stop_accepting: StopAccepting { token: stop_token } },
    ))
}

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeServer;

/// The bound listener for the IPC server.
///
/// Unix: a Tokio [`UnixListener`](tokio::net::UnixListener) bound inside a
/// [`NamedTempFile`](tempfile::NamedTempFile) so its socket file is unlinked
/// on `Drop`. Windows: a single named-pipe instance that is created up front
/// and replaced on each `accept` (a new pipe instance must be created before
/// the previous one is handed to the client, otherwise concurrent connect
/// attempts race for it).
#[cfg(unix)]
struct Bound {
    file: tempfile::NamedTempFile<tokio::net::UnixListener>,
}

#[cfg(windows)]
struct Bound {
    pipe_name: OsString,
    pending: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(unix)]
fn bind_listener() -> io::Result<(OsString, Bound)> {
    // `make` lets us bind the socket directly to the path tempfile picks; the
    // closure is responsible for creating the file (`UnixListener::bind` does).
    // The `NamedTempFile` wrapper unlinks the socket path on `Drop`.
    let file = tempfile::Builder::new()
        .prefix("vite_task_ipc_")
        .make(|path| tokio::net::UnixListener::bind(path))?;
    let name = file.path().as_os_str().to_owned();
    Ok((name, Bound { file }))
}

#[cfg(windows)]
fn bind_listener() -> io::Result<(OsString, Bound)> {
    use tokio::net::windows::named_pipe::ServerOptions;

    #[expect(
        clippy::disallowed_macros,
        reason = "pipe name always exceeds Str inline capacity; format! is the simplest construction"
    )]
    let pipe_name = OsString::from(format!(r"\\.\pipe\vite_task_ipc_{}", uuid::Uuid::new_v4()));
    let pending = ServerOptions::new().first_pipe_instance(true).create(&pipe_name)?;
    Ok((pipe_name.clone(), Bound { pipe_name, pending }))
}

impl Bound {
    #[cfg(unix)]
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "Windows variant requires &mut self to swap pending instance; keep the signature uniform across cfgs so `run` can call it identically."
    )]
    async fn accept(&mut self) -> io::Result<Stream> {
        let (stream, _addr) = self.file.as_file().accept().await?;
        Ok(stream)
    }

    #[cfg(windows)]
    async fn accept(&mut self) -> io::Result<Stream> {
        use tokio::net::windows::named_pipe::ServerOptions;

        // Wait for the next client to connect to the currently-pending
        // instance, then immediately create a fresh instance to listen for the
        // connection after that. Creating the next instance before yielding the
        // accepted one ensures no client gets `ERROR_PIPE_BUSY` during the
        // handoff.
        self.pending.connect().await?;
        let next = ServerOptions::new().create(&self.pipe_name)?;
        Ok(std::mem::replace(&mut self.pending, next))
    }
}

async fn run<H: Handler>(
    mut bound: Bound,
    handler: &RefCell<H>,
    shutdown: CancellationToken,
) -> Option<Error> {
    let mut clients = FuturesUnordered::new();
    let mut first_err: Option<Error> = None;

    // Accept phase: accept new clients until shutdown fires.
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            accept_result = bound.accept() => {
                match accept_result {
                    Ok(stream) => {
                        clients.push(handle_client(stream, handler).boxed_local());
                    }
                    Err(err) => {
                        tracing::warn!(?err, "vite_task_server: accept failed");
                    }
                }
            }
            Some(result) = clients.next(), if !clients.is_empty() => {
                if let Err(err) = result
                    && first_err.is_none()
                {
                    first_err = Some(err);
                    shutdown.cancel();
                }
            }
        }
    }

    // Stop accepting: drop the listener (and on Unix unlink the socket file).
    // Existing client streams continue to work.
    drop(bound);

    // Drain phase: wait for all in-flight per-client tasks to finish.
    while let Some(result) = clients.next().await {
        if let Err(err) = result
            && first_err.is_none()
        {
            first_err = Some(err);
        }
    }

    first_err
}

async fn handle_client<H: Handler>(mut stream: Stream, handler: &RefCell<H>) -> Result<(), Error> {
    let mut buf = Vec::new();
    loop {
        match read_frame(&mut stream, &mut buf).await {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(Error::ReadFrame(err)),
        }

        let request: Request<'_> =
            wincode::deserialize_exact(&buf).map_err(Error::InvalidRequest)?;

        // The fire-and-forget branch (`DisableCache`) intentionally writes no
        // response. Nothing in the runner observes individual IPC events
        // live; the recorded set is collected after this driver drains. See
        // `Request` in `vite_task_ipc_shared` for the rationale.
        match request {
            Request::DisableCache => {
                handler.borrow_mut().disable_cache();
            }
            Request::GetEnv { name } => {
                let value = handler.borrow_mut().get_env(name.to_cow_os_str().as_ref());
                let response = GetEnvResponse { env_value: value.as_deref().map(Into::into) };
                write_response(&mut stream, &response).await.map_err(Error::WriteResponse)?;
            }
        }
    }
}

async fn read_frame(stream: &mut Stream, buf: &mut Vec<u8>) -> io::Result<()> {
    let mut len_bytes = [0u8; 4];
    stream.read_exact(&mut len_bytes).await?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    buf.clear();
    buf.resize(len, 0);
    stream.read_exact(buf).await?;
    Ok(())
}

async fn write_response<T>(stream: &mut Stream, response: &T) -> io::Result<()>
where
    T: SchemaWrite<DefaultConfig, Src = T> + ?Sized,
{
    let bytes = wincode::serialize(response)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let len = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "response too large"))?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

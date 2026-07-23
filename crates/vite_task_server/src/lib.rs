use std::{
    cell::RefCell,
    ffi::{OsStr, OsString},
    io,
    sync::Arc,
};

use futures::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use native_str::NativeStr;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use vite_ipc::{Server as TransportServer, ServerConnection as Stream};
use vite_path::AbsolutePath;
use vite_task_ipc_shared::{
    EnvQuery as IpcEnvQuery, GetEnvResponse, GetEnvsResponse, IPC_ENV_NAME, Request,
};
use wincode::{SchemaWrite, config::DefaultConfig};

pub trait Handler {
    fn ignore_input(&mut self, path: &Arc<AbsolutePath>);
    fn ignore_output(&mut self, path: &Arc<AbsolutePath>);
    fn disable_cache(&mut self);
    fn get_env(&mut self, name: &OsStr, tracked: bool) -> Option<Arc<OsStr>>;
    /// Returns the subset of the env map whose names match `query`.
    ///
    /// # Errors
    ///
    /// Returns an error if a glob query fails to parse.
    fn get_envs(
        &mut self,
        query: &IpcEnvQuery<'_>,
        tracked: bool,
    ) -> Result<FxHashMap<Arc<OsStr>, Arc<OsStr>>, vite_glob::env::EnvGlobError>;
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

    #[error("non-absolute path from the task: {path:?}")]
    NonAbsolutePath { path: OsString },

    #[error("invalid glob pattern from the task: {:?}", .0.pattern)]
    InvalidGlob(Box<InvalidGlob>),

    #[error("failed to send response to the task")]
    WriteResponse(#[source] io::Error),
}

/// Payload for [`Error::InvalidGlob`]. Boxed so the `Error` enum stays small.
#[derive(Debug)]
pub struct InvalidGlob {
    pub pattern: Box<str>,
    pub source: vite_glob::env::EnvGlobError,
}

/// A [`Handler`] that records cache-relevant reports and resolves env requests
/// against provided envs.
///
/// Call [`Recorder::into_reports`] after the driver future completes to
/// recover the collected [`Reports`].
pub struct Recorder {
    ignored_inputs: FxHashSet<Arc<AbsolutePath>>,
    ignored_outputs: FxHashSet<Arc<AbsolutePath>>,
    cache_disabled: bool,
    tracked_get_env: FxHashMap<Arc<OsStr>, Option<Arc<OsStr>>>,
    tracked_get_envs: FxHashMap<EnvQuery, EnvQueryRecord>,
    /// The envs `get_env` resolves against. The runner supplies these for the
    /// spawned task; the server never re-reads the live process env.
    envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>>,
}

/// Owned env query key recorded for tracked `get_envs` calls.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EnvQuery {
    Glob(Arc<str>),
    Prefix(Arc<str>),
}

/// A record of a tracked env query made via `get_envs`.
///
/// `matches` is captured on the first call and reused on repeat queries; the
/// server's env map is immutable for a task's lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvQueryRecord {
    pub matches: FxHashMap<Arc<OsStr>, Arc<OsStr>>,
}

/// The data collected by a [`Recorder`] over the server's lifetime.
#[derive(Debug, Default)]
pub struct Reports {
    pub ignored_inputs: FxHashSet<Arc<AbsolutePath>>,
    pub ignored_outputs: FxHashSet<Arc<AbsolutePath>>,
    pub cache_disabled: bool,
    pub tracked_get_env: FxHashMap<Arc<OsStr>, Option<Arc<OsStr>>>,
    pub tracked_get_envs: FxHashMap<EnvQuery, EnvQueryRecord>,
}

impl Recorder {
    #[must_use]
    pub fn new(envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>>) -> Self {
        Self {
            ignored_inputs: FxHashSet::default(),
            ignored_outputs: FxHashSet::default(),
            cache_disabled: false,
            tracked_get_env: FxHashMap::default(),
            tracked_get_envs: FxHashMap::default(),
            envs,
        }
    }

    #[must_use]
    pub fn into_reports(self) -> Reports {
        Reports {
            ignored_inputs: self.ignored_inputs,
            ignored_outputs: self.ignored_outputs,
            cache_disabled: self.cache_disabled,
            tracked_get_env: self.tracked_get_env,
            tracked_get_envs: self.tracked_get_envs,
        }
    }
}

impl Handler for Recorder {
    fn ignore_input(&mut self, path: &Arc<AbsolutePath>) {
        self.ignored_inputs.insert(Arc::clone(path));
    }

    fn ignore_output(&mut self, path: &Arc<AbsolutePath>) {
        self.ignored_outputs.insert(Arc::clone(path));
    }

    fn disable_cache(&mut self) {
        self.cache_disabled = true;
    }

    fn get_env(&mut self, name: &OsStr, tracked: bool) -> Option<Arc<OsStr>> {
        let value = self.envs.get(name).cloned();
        if tracked {
            self.tracked_get_env.entry(name.into()).or_insert_with(|| value.clone());
        }
        value
    }

    fn get_envs(
        &mut self,
        query: &IpcEnvQuery<'_>,
        tracked: bool,
    ) -> Result<FxHashMap<Arc<OsStr>, Arc<OsStr>>, vite_glob::env::EnvGlobError> {
        let key = match query {
            IpcEnvQuery::Glob(pattern) => EnvQuery::Glob(Arc::from(*pattern)),
            IpcEnvQuery::Prefix(prefix) => EnvQuery::Prefix(Arc::from(*prefix)),
        };
        if let Some(existing) = self.tracked_get_envs.get(&key) {
            return Ok(existing.matches.clone());
        }
        let matches: FxHashMap<Arc<OsStr>, Arc<OsStr>> = match query {
            IpcEnvQuery::Glob(pattern) => {
                let glob = vite_glob::env::EnvGlob::new(pattern)?;
                self.envs
                    .iter()
                    .filter_map(|(name, value)| {
                        let name_str = name.to_str()?;
                        if glob.is_match(name_str) {
                            Some((Arc::clone(name), Arc::clone(value)))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            IpcEnvQuery::Prefix(prefix) => self
                .envs
                .iter()
                .filter_map(|(name, value)| {
                    let name_str = name.to_str()?;
                    if env_name_starts_with(name_str, prefix) {
                        Some((Arc::clone(name), Arc::clone(value)))
                    } else {
                        None
                    }
                })
                .collect(),
        };
        if tracked {
            self.tracked_get_envs.insert(key, EnvQueryRecord { matches: matches.clone() });
        }
        Ok(matches)
    }
}

#[cfg(not(windows))]
fn env_name_starts_with(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix)
}

#[cfg(windows)]
fn env_name_starts_with(name: &str, prefix: &str) -> bool {
    let mut name_chars = name.chars();
    for prefix_char in prefix.chars() {
        let Some(name_char) = name_chars.next() else {
            return false;
        };
        if !name_char.eq_ignore_ascii_case(&prefix_char) {
            return false;
        }
    }
    true
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
/// Returns an error if creating the transport server fails.
pub fn serve<'h, H: Handler + 'h>(
    handler: H,
) -> io::Result<(impl Iterator<Item = (&'static OsStr, OsString)>, ServerHandle<'h, H>)> {
    let stop_token = CancellationToken::new();
    let server = TransportServer::bind()?;
    let name = server.name().to_owned();

    let run_stop = stop_token.clone();
    let driver = async move {
        // Multiple per-client futures coexist inside `FuturesUnordered` and each
        // calls `&mut self` handler methods. `RefCell` provides the interior
        // mutability that makes these shared-access method calls compile; at
        // runtime the `borrow_mut()` never conflicts because we're on a
        // single-threaded runtime and handler methods are synchronous (no
        // awaits, so no borrow spans a yield point).
        let handler = RefCell::new(handler);
        let first_err = run(server, &handler, run_stop).await;
        first_err.map_or_else(|| Ok(handler.into_inner()), Err)
    }
    .boxed_local();

    Ok((
        std::iter::once((OsStr::new(IPC_ENV_NAME), name)),
        ServerHandle { driver, stop_accepting: StopAccepting { token: stop_token } },
    ))
}

async fn run<H: Handler>(
    mut server: TransportServer,
    handler: &RefCell<H>,
    shutdown: CancellationToken,
) -> Option<Error> {
    let mut clients = FuturesUnordered::new();
    let mut first_err: Option<Error> = None;

    // Accept phase: accept new clients until shutdown fires.
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            accept_result = server.accept() => {
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

    // Stop accepting. Existing client streams continue to work.
    drop(server);

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

        // Fire-and-forget branches (`IgnoreInput`, `IgnoreOutput`, `DisableCache`)
        // intentionally write no response. Nothing in the runner observes
        // individual IPC events live; the recorded set is collected after
        // this driver drains. See `Request` in `vite_task_ipc_shared` for
        // the rationale.
        match request {
            Request::IgnoreInput(ns) => {
                let path = native_str_to_abs_path(ns)?;
                handler.borrow_mut().ignore_input(&path);
            }
            Request::IgnoreOutput(ns) => {
                let path = native_str_to_abs_path(ns)?;
                handler.borrow_mut().ignore_output(&path);
            }
            Request::DisableCache => {
                handler.borrow_mut().disable_cache();
            }
            Request::GetEnv { name, tracked } => {
                let value = handler.borrow_mut().get_env(name.to_cow_os_str().as_ref(), tracked);
                let response = GetEnvResponse { env_value: value.as_deref().map(Into::into) };
                write_response(&mut stream, &response).await.map_err(Error::WriteResponse)?;
            }
            Request::GetEnvs { query, tracked } => {
                let matches = handler.borrow_mut().get_envs(&query, tracked).map_err(|source| {
                    let pattern = match query {
                        IpcEnvQuery::Glob(pattern) => pattern,
                        IpcEnvQuery::Prefix(prefix) => prefix,
                    };
                    Error::InvalidGlob(Box::new(InvalidGlob {
                        pattern: Box::<str>::from(pattern),
                        source,
                    }))
                })?;
                let response = GetEnvsResponse {
                    entries: matches.iter().map(|(k, v)| ((&**k).into(), (&**v).into())).collect(),
                };
                write_response(&mut stream, &response).await.map_err(Error::WriteResponse)?;
            }
        }
    }
}

fn native_str_to_abs_path(ns: &NativeStr) -> Result<Arc<AbsolutePath>, Error> {
    let os_str = ns.to_cow_os_str();
    AbsolutePath::new(&*os_str)
        .map(Arc::from)
        .ok_or_else(|| Error::NonAbsolutePath { path: os_str.into_owned() })
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

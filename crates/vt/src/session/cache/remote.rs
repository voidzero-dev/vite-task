//! The remote cache tier. After a local miss, the entry is fetched from the
//! remote cache, and after a local update in `read-write` mode, the entry is
//! uploaded to it. Entries are opaque bytes to the remote cache:
//!
//! | Field           | Contents                              |
//! | --------------- | ------------------------------------- |
//! | `key`           | Header + wincode(`CacheEntryKey`)     |
//! | `secondary_key` | Header + wincode(`ExecutionCacheKey`) |
//! | `value`         | wincode(`CacheEntryValue`)            |
//! | blob            | The `.tar.zst` output archive         |
//!
//! Both keys start with a header containing the cache schema version and the
//! target OS and architecture. Platforms share an endpoint's namespace, but
//! their keys differ.

use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Write as _},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use vt_casefold::EnvName;
use vt_path::AbsolutePath;
use vt_plan::cache_metadata::ExecutionCacheKey;
use vt_remote_cache::{Client, Download, Fetched, GithubOidc, StoreAuth};
use vt_str::Str;
use wincode::{
    SchemaWrite,
    error::{WriteError, WriteResult},
};

use super::{
    CACHE_SCHEMA_VERSION, CacheEntryKey, CacheEntryValue, CacheMiss, FingerprintMismatch,
    TaskCacheConfig, archive, deserialize_cache, serialize_cache,
};

/// Why an entry wasn't uploaded.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error(transparent)]
    Remote(#[from] vt_remote_cache::Error),
    /// Uploads to the endpoint aren't authorized: no GitHub Actions OIDC token
    /// could be obtained, or the server responded with 401 or 403. Uploads to
    /// the endpoint stop, and each later one returns the error that stopped
    /// them without a request, so they all report the same reason.
    #[error(transparent)]
    Unauthorized(Arc<vt_remote_cache::Error>),
    #[error("failed to encode the cache entry")]
    Encode(#[from] WriteError),
    /// The run was cancelled, by Ctrl-C or fast-fail, before the upload
    /// finished.
    #[error("cancelled")]
    Cancelled,
}

/// Why no entry could be read from the remote cache. It's a cache miss, and
/// the message is its reason. The message names only the kind of failure, so
/// it's the same on every platform.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("remote cache fetch failed")]
    Fetch(#[source] vt_remote_cache::Error),
    #[error("remote cache download failed")]
    Download(#[source] vt_remote_cache::Error),
    #[error("remote cache value is corrupt")]
    CorruptValue(#[source] wincode::error::ReadError),
    #[error("remote cache key is corrupt")]
    CorruptKey(#[source] Option<wincode::error::ReadError>),
    /// The value has an output archive but the entry has no blob, or the
    /// other way around.
    #[error("remote cache entry's blob doesn't match its value")]
    MismatchedBlob,
    #[error("downloaded archive is corrupt")]
    CorruptArchive(#[source] io::Error),
    #[error("failed to write the downloaded archive")]
    WriteArchive(#[source] io::Error),
    #[error("remote cache entry couldn't be validated")]
    Validate(#[source] anyhow::Error),
    #[error("failed to encode the cache key")]
    Encode(#[from] WriteError),
    /// The run was cancelled, by Ctrl-C or fast-fail, before the read
    /// finished. The task doesn't start then, so this miss isn't reported.
    #[error("cancelled")]
    Cancelled,
}

impl ReadError {
    pub(super) fn into_miss(self) -> CacheMiss {
        CacheMiss::RemoteReadFailed(Arc::new(self))
    }
}

/// An exact entry that passed validation. It's a hit once its blob, if any,
/// is downloaded.
#[derive(Debug)]
pub(super) struct Restore {
    pub value: CacheEntryValue,
    pub blob_id: Option<Str>,
}

/// Turn the result of a fetch into an entry to restore or a miss. `validate`
/// checks an exact entry against the current execution. A fallback's miss
/// reason compares its key with `cache_key`, and no match is a miss without
/// an entry. A failed fetch, an entry that doesn't decode or whose blob
/// doesn't match its value, or a validation error is a read failure.
#[expect(
    clippy::result_large_err,
    reason = "`CacheMiss` is intentionally large, and a lookup returns it once"
)]
pub(super) fn resolve(
    fetched: Result<Option<Fetched>, ReadError>,
    cache_key: &CacheEntryKey,
    validate: impl FnOnce(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>>,
) -> Result<Restore, CacheMiss> {
    let (value, blob_id) = match fetched.map_err(ReadError::into_miss)? {
        Some(Fetched::Exact { value, blob_id }) => {
            let value: CacheEntryValue = deserialize_cache(&value)
                .map_err(|err| ReadError::CorruptValue(err).into_miss())?;
            if value.output_archive.is_some() != blob_id.is_some() {
                return Err(ReadError::MismatchedBlob.into_miss());
            }
            (value, blob_id)
        }
        Some(Fetched::Fallback { key }) => {
            let key = decode_key(&key).map_err(ReadError::into_miss)?;
            return Err(CacheMiss::FingerprintMismatch(key.into_mismatch(cache_key)));
        }
        None => return Err(CacheMiss::NotFound),
    };
    match validate(&value) {
        Ok(None) => Ok(Restore { value, blob_id }),
        Ok(Some(mismatch)) => Err(CacheMiss::FingerprintMismatch(mismatch)),
        Err(err) => Err(ReadError::Validate(err).into_miss()),
    }
}

/// How uploads authenticate, from the session envs. A GitHub Actions job
/// granted `id-token: write` has both OIDC token request variables, and its
/// uploads send a token. These variables are also passed through to tasks,
/// so tools such as npm can use them too.
pub fn store_auth(envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>) -> StoreAuth {
    let env = |name: &str| {
        envs.get(EnvName::from_ref(OsStr::new(name)))
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
    };
    match (env("ACTIONS_ID_TOKEN_REQUEST_URL"), env("ACTIONS_ID_TOKEN_REQUEST_TOKEN")) {
        (Some(request_url), Some(request_token)) => {
            StoreAuth::GithubOidc(GithubOidc::new(request_url, request_token))
        }
        _ if env("GITHUB_ACTIONS") == Some("true") => StoreAuth::GithubActionsWithoutOidc,
        _ => StoreAuth::Anonymous,
    }
}

/// Remote cache clients, each created when its endpoint is first used.
#[derive(Debug, Default)]
pub struct RemoteClients {
    store_auth: StoreAuth,
    endpoints: Mutex<FxHashMap<Arc<str>, Arc<Endpoint>>>,
}

#[derive(Debug)]
struct Endpoint {
    client: Client,
    /// The error that stopped uploads to the endpoint. See
    /// [`UploadError::Unauthorized`].
    uploads_stopped: OnceLock<Arc<vt_remote_cache::Error>>,
}

impl RemoteClients {
    /// Clients whose uploads authenticate with `store_auth`.
    pub fn new(store_auth: StoreAuth) -> Self {
        Self { store_auth, endpoints: Mutex::default() }
    }

    fn endpoint(&self, endpoint: &Arc<str>) -> Result<Arc<Endpoint>, vt_remote_cache::Error> {
        let mut endpoints = self.endpoints.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = endpoints.get(endpoint) {
            return Ok(Arc::clone(existing));
        }
        let created = Arc::new(Endpoint {
            client: Client::new(endpoint, self.store_auth.clone())?,
            uploads_stopped: OnceLock::new(),
        });
        endpoints.insert(Arc::clone(endpoint), Arc::clone(&created));
        drop(endpoints);
        Ok(created)
    }

    /// Fetch the entry stored under `cache_key`, falling back to the entry
    /// last stored for `execution_cache_key`. Returns `None` if neither key
    /// matched. Stops when `cancel_token` is cancelled.
    pub(super) async fn fetch(
        &self,
        endpoint: &Arc<str>,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cancel_token: &CancellationToken,
    ) -> Result<Option<Fetched>, ReadError> {
        let endpoint = self.endpoint(endpoint).map_err(ReadError::Fetch)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        cancel_token
            .run_until_cancelled(endpoint.client.fetch(&key, &secondary_key))
            .await
            .ok_or(ReadError::Cancelled)?
            .map_err(ReadError::Fetch)
    }

    /// Download the blob `blob_id` into `cache_dir`, checking that it decodes
    /// as an output archive as it arrives. It's downloaded to a `.tmp` file,
    /// which is renamed once the check passes and removed otherwise, such as
    /// when `cancel_token` is cancelled. Returns the archive's file name.
    pub(super) async fn download_archive(
        &self,
        endpoint: &Arc<str>,
        blob_id: &str,
        cache_dir: &AbsolutePath,
        cancel_token: &CancellationToken,
    ) -> Result<Str, ReadError> {
        let endpoint = self.endpoint(endpoint).map_err(ReadError::Download)?;
        let archive_name = vt_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
        let archive_path = cache_dir.join(archive_name.as_str());
        let temp_path = cache_dir.join(vt_str::format!("{archive_name}.tmp").as_str());
        let result = download_checked(&endpoint.client, blob_id, &temp_path, cancel_token)
            .await
            .and_then(|()| {
                std::fs::rename(temp_path.as_path(), archive_path.as_path())
                    .map_err(ReadError::WriteArchive)
            });
        if result.is_err() {
            // Best-effort cleanup: the file may not have been created.
            let _ = std::fs::remove_file(temp_path.as_path());
        }
        result.map(|()| archive_name)
    }

    /// Upload an entry that was just recorded locally, along with its output
    /// archive in `cache_dir`. Stops when `cancel_token` is cancelled. Once an
    /// upload to the endpoint is unauthorized, later ones return its error
    /// without a request.
    pub(super) async fn upload(
        &self,
        endpoint: &Arc<str>,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cache_value: &CacheEntryValue,
        cache_dir: &AbsolutePath,
        cancel_token: &CancellationToken,
    ) -> Result<(), UploadError> {
        let endpoint = self.endpoint(endpoint)?;
        if let Some(err) = endpoint.uploads_stopped.get() {
            return Err(UploadError::Unauthorized(Arc::clone(err)));
        }
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        let value = serialize_cache(cache_value)?;
        let archive = cache_value.output_archive.as_ref().map(|name| cache_dir.join(name.as_str()));
        let store = endpoint.client.store(&key, &secondary_key, &value, archive.as_deref());
        match cancel_token.run_until_cancelled(store).await.ok_or(UploadError::Cancelled)? {
            Ok(()) => Ok(()),
            Err(err) if err.is_unauthorized() => {
                let err = Arc::new(err);
                // Concurrent uploads may fail the same way. The first to
                // finish stops the rest.
                let _ = endpoint.uploads_stopped.set(Arc::clone(&err));
                Err(UploadError::Unauthorized(err))
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Chunks buffered between the download and the archive check.
const CHECK_BUFFER_CHUNKS: usize = 16;

/// Download the blob `blob_id` to the file at `path`, until `cancel_token` is
/// cancelled. The chunks flow one way: from the network to the archive check
/// on a blocking thread, which writes each one to the file as it takes it.
async fn download_checked(
    client: &Client,
    blob_id: &str,
    path: &AbsolutePath,
    cancel_token: &CancellationToken,
) -> Result<(), ReadError> {
    let download = cancel_token
        .run_until_cancelled(client.download(blob_id))
        .await
        .ok_or(ReadError::Cancelled)?
        .map_err(ReadError::Download)?;
    let file = File::create(path.as_path()).map_err(ReadError::WriteArchive)?;
    let (sender, receiver) = mpsc::channel(CHECK_BUFFER_CHUNKS);
    let check = tokio::task::spawn_blocking(move || {
        let mut reader = DownloadReader { receiver, chunk: Bytes::new(), file, write_error: None };
        let checked = archive::check_output_archive(&mut reader);
        if let Some(err) = reader.write_error {
            return Err(ReadError::WriteArchive(err));
        }
        checked.map_err(ReadError::CorruptArchive)
    });
    // Cancelling drops the sender, which ends the check.
    let sent = cancel_token.run_until_cancelled(send_chunks(download, sender)).await;
    // Wait for the check even after a network error or cancellation, so the
    // file is closed before the caller removes it.
    let checked = check.await.unwrap_or_else(|err| Err(ReadError::CorruptArchive(err.into())));
    sent.ok_or(ReadError::Cancelled)?.map_err(ReadError::Download)?;
    checked
}

/// Send the blob's chunks through `sender` until the blob ends or the receiver
/// is dropped. The check drops the receiver when it fails, which stops the
/// download without waiting for the next chunk.
async fn send_chunks(
    mut download: Download,
    sender: mpsc::Sender<Bytes>,
) -> Result<(), vt_remote_cache::Error> {
    loop {
        let chunk = tokio::select! {
            chunk = download.chunk() => chunk?,
            () = sender.closed() => break,
        };
        let Some(chunk) = chunk else { break };
        if sender.send(chunk).await.is_err() {
            break;
        }
    }
    Ok(())
}

/// Reads the chunks sent through a channel, in order, until the sender is
/// dropped, and writes each one to `file` as it takes it. Reading blocks, so
/// it's only for blocking threads.
struct DownloadReader {
    receiver: mpsc::Receiver<Bytes>,
    chunk: Bytes,
    file: File,
    /// Why writing to `file` failed. Reading fails after it, and the check's
    /// error is then just a consequence.
    write_error: Option<io::Error>,
}

impl io::Read for DownloadReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.chunk.is_empty() {
            let Some(chunk) = self.receiver.blocking_recv() else {
                return Ok(0);
            };
            if let Err(err) = self.file.write_all(&chunk) {
                self.write_error = Some(err);
                return Err(io::ErrorKind::Other.into());
            }
            self.chunk = chunk;
        }
        let len = buf.len().min(self.chunk.len());
        buf[..len].copy_from_slice(&self.chunk.split_to(len));
        Ok(len)
    }
}

#[derive(SchemaWrite)]
struct KeyHeader {
    cache_schema_version: u32,
    os: Str,
    arch: Str,
}

/// The key header for this build.
fn encode_header() -> WriteResult<Vec<u8>> {
    serialize_cache(&KeyHeader {
        cache_schema_version: CACHE_SCHEMA_VERSION,
        os: Str::from(std::env::consts::OS),
        arch: Str::from(std::env::consts::ARCH),
    })
}

/// Encode a key with the header for this build.
fn encode_key<K: SchemaWrite<TaskCacheConfig, Src = K>>(key: &K) -> WriteResult<Vec<u8>> {
    let mut bytes = encode_header()?;
    bytes.extend(serialize_cache(key)?);
    Ok(bytes)
}

/// Decode a key stored with the header for this build.
fn decode_key(bytes: &[u8]) -> Result<CacheEntryKey, ReadError> {
    let header = encode_header()?;
    let key = bytes.strip_prefix(header.as_slice()).ok_or(ReadError::CorruptKey(None))?;
    deserialize_cache(key).map_err(|err| ReadError::CorruptKey(Some(err)))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io::Read as _, net::TcpListener, time::Duration};

    use tokio::sync::oneshot;
    use vt_graph::config::ResolvedGlobConfig;
    use vt_path::{AbsolutePathBuf, RelativePathBuf};
    use vt_plan::cache_metadata::{EnvValueHash, SpawnFingerprint};

    use super::*;
    use crate::session::{
        cache::InputChangeKind,
        execute::{
            fingerprint::{PostRunFingerprint, TrackedEnvQuery},
            pipe::{OutputKind, StdOutput},
        },
    };

    /// A key whose spawn fingerprint runs `vtt build`. `SpawnFingerprint`'s
    /// fields are private to `vt_plan`, so it's decoded from types with the
    /// same encoding. Update them if `SpawnFingerprint` changes.
    fn cache_key(input_config: ResolvedGlobConfig) -> CacheEntryKey {
        #[derive(SchemaWrite)]
        enum ProgramFingerprintLayout {
            OutsideWorkspace { program_name: Str },
        }
        #[derive(SchemaWrite)]
        struct SpawnFingerprintLayout {
            cwd: RelativePathBuf,
            program_fingerprint: ProgramFingerprintLayout,
            args: Arc<[Str]>,
            fingerprinted_envs: BTreeMap<Str, EnvValueHash>,
            untracked_env_config: Arc<[Str]>,
        }

        let layout = SpawnFingerprintLayout {
            cwd: RelativePathBuf::default(),
            program_fingerprint: ProgramFingerprintLayout::OutsideWorkspace {
                program_name: Str::from("vtt"),
            },
            args: Arc::from([Str::from("build")]),
            fingerprinted_envs: BTreeMap::new(),
            untracked_env_config: Arc::from([]),
        };
        let spawn_fingerprint: SpawnFingerprint =
            deserialize_cache(&serialize_cache(&layout).unwrap()).unwrap();
        CacheEntryKey {
            spawn_fingerprint,
            input_config,
            output_config: ResolvedGlobConfig::default_auto(),
        }
    }

    fn cache_value() -> CacheEntryValue {
        CacheEntryValue {
            post_run_fingerprint: PostRunFingerprint::default(),
            std_outputs: Arc::new([StdOutput {
                kind: OutputKind::StdOut,
                content: b"built\n".to_vec(),
            }]),
            duration: Duration::from_millis(5),
            globbed_inputs: BTreeMap::new(),
            output_archive: Some(Str::from("uploader.tar.zst")),
        }
    }

    fn exact(value: &CacheEntryValue) -> Fetched {
        Fetched::Exact { value: serialize_cache(value).unwrap(), blob_id: Some(Str::from("1")) }
    }

    /// Validate as `try_hit_remote` does, with no envs and `globbed_inputs` as
    /// the current inputs.
    fn validate_against(
        globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    ) -> impl FnOnce(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>> {
        move |value| {
            let workspace_root = vt_path::current_dir().unwrap();
            value.validate(&FxHashMap::default(), &globbed_inputs, &workspace_root)
        }
    }

    fn not_validated(_: &CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>> {
        panic!("only exact entries that decode and match their blob are validated")
    }

    fn read_failure(miss: CacheMiss) -> Str {
        match miss {
            CacheMiss::RemoteReadFailed(error) => vt_str::format!("{error}"),
            miss => panic!("expected a read failure, got {miss:?}"),
        }
    }

    #[test]
    fn exact_entry_that_validates_is_restored() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let restore =
            resolve(Ok(Some(exact(&cache_value()))), &key, validate_against(BTreeMap::new()))
                .unwrap();
        assert_eq!(restore.blob_id.as_deref(), Some("1"));
        assert_eq!(restore.value.std_outputs[0].content, b"built\n");
        assert_eq!(restore.value.duration, Duration::from_millis(5));
    }

    #[test]
    fn exact_entry_that_fails_validation_is_a_mismatch() {
        let current_inputs = BTreeMap::from([(RelativePathBuf::new("src/a.txt").unwrap(), 1)]);
        let miss = resolve(
            Ok(Some(exact(&cache_value()))),
            &cache_key(ResolvedGlobConfig::default_auto()),
            validate_against(current_inputs),
        )
        .unwrap_err();
        assert!(
            matches!(
                &miss,
                CacheMiss::FingerprintMismatch(FingerprintMismatch::InputChanged {
                    kind: InputChangeKind::Added,
                    path,
                }) if path.as_str() == "src/a.txt"
            ),
            "{miss:?}"
        );
    }

    #[test]
    fn exact_entry_that_cannot_be_validated_is_a_read_failure() {
        // The stored env query isn't a valid glob, so validating it errors.
        let mut value = cache_value();
        value
            .post_run_fingerprint
            .tracked_env_queries
            .insert(TrackedEnvQuery::Glob(Str::from("PROBE_[")), BTreeMap::new());
        let miss = resolve(
            Ok(Some(exact(&value))),
            &cache_key(ResolvedGlobConfig::default_auto()),
            validate_against(BTreeMap::new()),
        )
        .unwrap_err();
        assert_eq!(read_failure(miss), "remote cache entry couldn't be validated");
    }

    #[test]
    fn fallback_is_a_mismatch_with_the_stored_key() {
        let stored_key = encode_key(&cache_key(ResolvedGlobConfig::default_auto())).unwrap();
        let mut input_config = ResolvedGlobConfig::default_auto();
        input_config.positive_globs.insert(Str::from("src/**"));
        let miss = resolve(
            Ok(Some(Fetched::Fallback { key: stored_key })),
            &cache_key(input_config),
            not_validated,
        )
        .unwrap_err();
        assert!(
            matches!(miss, CacheMiss::FingerprintMismatch(FingerprintMismatch::InputConfig)),
            "{miss:?}"
        );
    }

    #[test]
    fn no_match_is_a_miss_without_an_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let miss = resolve(Ok(None), &key, not_validated).unwrap_err();
        assert!(matches!(miss, CacheMiss::NotFound), "{miss:?}");
    }

    #[test]
    fn failed_fetch_is_a_read_failure() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let fetched = Err(ReadError::Fetch(vt_remote_cache::Error::InvalidEndpoint(None)));
        let miss = resolve(fetched, &key, not_validated).unwrap_err();
        assert_eq!(read_failure(miss), "remote cache fetch failed");
    }

    #[test]
    fn value_that_does_not_decode_is_a_corrupt_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let mut trailing = serialize_cache(&cache_value()).unwrap();
        trailing.push(0);
        for value in [b"not a cache value".to_vec(), trailing] {
            let fetched = Ok(Some(Fetched::Exact { value, blob_id: None }));
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache value is corrupt");
        }
    }

    #[test]
    fn blob_that_does_not_match_the_value_is_a_read_failure() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let without_archive = CacheEntryValue { output_archive: None, ..cache_value() };
        for (value, blob_id) in [(cache_value(), None), (without_archive, Some(Str::from("1")))] {
            let value = serialize_cache(&value).unwrap();
            let fetched = Ok(Some(Fetched::Exact { value, blob_id }));
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache entry's blob doesn't match its value");
        }
    }

    #[test]
    fn fallback_key_that_does_not_decode_is_a_corrupt_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let mut other_header = serialize_cache(&KeyHeader {
            cache_schema_version: CACHE_SCHEMA_VERSION + 1,
            os: Str::from(std::env::consts::OS),
            arch: Str::from(std::env::consts::ARCH),
        })
        .unwrap();
        other_header.extend(serialize_cache(&key).unwrap());
        let mut garbage = encode_header().unwrap();
        garbage.extend(b"not a cache key");
        for stored_key in [b"not a cache key".to_vec(), other_header, garbage] {
            let fetched = Ok(Some(Fetched::Fallback { key: stored_key }));
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache key is corrupt");
        }
    }

    /// Serve one request on a loopback endpoint: once the request head
    /// arrives, write `response`, then send nothing more and keep the
    /// connection open until the client closes it. Returns the endpoint and a
    /// receiver that resolves once `response` is written.
    fn serve_stalled(response: &'static [u8]) -> (Arc<str>, oneshot::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint =
            Arc::from(vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let (responded_sender, responded) = oneshot::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0, "connection closed before the request head ended");
                request.extend_from_slice(&buf[..n]);
            }
            stream.write_all(response).unwrap();
            let _ = responded_sender.send(());
            while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
        });
        (endpoint, responded)
    }

    #[tokio::test]
    async fn failed_archive_check_stops_the_download() {
        // The response announces more than it sends, so only the failed check
        // can end the download.
        let (endpoint, _) =
            serve_stalled(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\nnot an archive");
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();

        let error = RemoteClients::default()
            .download_archive(&endpoint, "1", &cache_dir, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, ReadError::CorruptArchive(_)), "{error:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancelling_stops_a_fetch() {
        let (endpoint, requested) = serve_stalled(b"");
        let cancel_token = CancellationToken::new();
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));

        let clients = RemoteClients::default();
        let fetch = clients.fetch(&endpoint, &key, &execution_key, &cancel_token);
        let (fetched, ()) = tokio::join!(fetch, async {
            requested.await.unwrap();
            cancel_token.cancel();
        });
        assert!(matches!(fetched, Err(ReadError::Cancelled)), "{fetched:?}");
    }

    #[tokio::test]
    async fn cancelling_stops_a_download_and_removes_it() {
        // The response announces a body that never arrives, so only
        // cancelling can end the download.
        let (endpoint, responded) =
            serve_stalled(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\n");
        let cancel_token = CancellationToken::new();
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();

        let clients = RemoteClients::default();
        let download = clients.download_archive(&endpoint, "1", &cache_dir, &cancel_token);
        let (downloaded, ()) = tokio::join!(download, async {
            responded.await.unwrap();
            // Cancel once the `.tmp` file exists, so the download has started
            // writing it.
            while std::fs::read_dir(dir.path()).unwrap().next().is_none() {
                tokio::task::yield_now().await;
            }
            cancel_token.cancel();
        });
        assert!(matches!(downloaded, Err(ReadError::Cancelled)), "{downloaded:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    fn envs(pairs: &[(&str, &str)]) -> FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>> {
        pairs
            .iter()
            .map(|(name, value)| {
                (EnvName::new(Arc::<OsStr>::from(OsStr::new(name))), Arc::from(OsStr::new(value)))
            })
            .collect()
    }

    #[test]
    fn store_auth_uses_oidc_when_both_variables_are_set() {
        let url = ("ACTIONS_ID_TOKEN_REQUEST_URL", "https://token.example/?api-version=2.0");
        let token = ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token");
        let github_actions = ("GITHUB_ACTIONS", "true");
        for (pairs, expected) in [
            (vec![url, token], "GithubOidc"),
            (vec![url, token, github_actions], "GithubOidc"),
            (
                vec![url, ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", ""), github_actions],
                "GithubActionsWithoutOidc",
            ),
            (vec![token, github_actions], "GithubActionsWithoutOidc"),
            (vec![github_actions], "GithubActionsWithoutOidc"),
            (vec![url], "Anonymous"),
            (vec![("GITHUB_ACTIONS", "false")], "Anonymous"),
            (vec![], "Anonymous"),
        ] {
            let auth = store_auth(&envs(&pairs));
            let kind = match auth {
                StoreAuth::Anonymous => "Anonymous",
                StoreAuth::GithubOidc(_) => "GithubOidc",
                StoreAuth::GithubActionsWithoutOidc => "GithubActionsWithoutOidc",
            };
            assert_eq!(kind, expected, "{pairs:?}");
        }
    }

    /// Serve one request for each of `responses` on a loopback server, write
    /// them in order, and close each connection. Returns the server's address
    /// and the raw requests.
    fn serve_each(responses: Vec<&'static [u8]>) -> (Str, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = vt_str::format!("{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    stream.write_all(response).unwrap();
                    request
                })
                .collect()
        });
        (address, server)
    }

    /// Read a request with a `content-length` body.
    fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        let head_end = loop {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request head ended");
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let content_length: usize = std::str::from_utf8(&request[..head_end])
            .unwrap()
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while request.len() < head_end + content_length {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request body ended");
            request.extend_from_slice(&buf[..n]);
        }
        request
    }

    async fn upload_to(clients: &RemoteClients, endpoint: &Arc<str>) -> Result<(), UploadError> {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        let value = CacheEntryValue { output_archive: None, ..cache_value() };
        let cache_dir = vt_path::current_dir().unwrap();
        clients
            .upload(endpoint, &key, &execution_key, &value, &cache_dir, &CancellationToken::new())
            .await
    }

    /// The message of `error` and each of its sources.
    fn messages(error: &UploadError) -> Vec<Str> {
        std::iter::successors(Some(error as &dyn std::error::Error), |err| err.source())
            .map(|err| vt_str::format!("{err}"))
            .collect()
    }

    #[tokio::test]
    async fn uploads_stop_once_one_is_unauthorized() {
        const TOKEN_FAILED: &[u8] =
            b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        const UNAUTHORIZED: &[u8] =
            b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 7\r\nconnection: close\r\n\r\nno auth";
        const FORBIDDEN: &[u8] =
            b"HTTP/1.1 403 Forbidden\r\ncontent-length: 6\r\nconnection: close\r\n\r\ndenied";
        for (github_actions, oidc, response, expected) in [
            (
                true,
                true,
                TOKEN_FAILED,
                ["failed to get a GitHub Actions OIDC token", "HTTP status 500"].as_slice(),
            ),
            (false, false, UNAUTHORIZED, &["HTTP status 401", "no auth"]),
            (false, false, FORBIDDEN, &["HTTP status 403", "denied"]),
            (
                true,
                false,
                UNAUTHORIZED,
                &["HTTP status 401", "grant `id-token: write` to this job", "no auth"],
            ),
        ] {
            let (address, server) = serve_each(vec![response]);
            let token_url = vt_str::format!("http://{address}/token?api-version=2.0");
            let mut pairs = vec![];
            if github_actions {
                pairs.push(("GITHUB_ACTIONS", "true"));
            }
            if oidc {
                pairs.push(("ACTIONS_ID_TOKEN_REQUEST_URL", token_url.as_str()));
                pairs.push(("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request-token"));
            }
            let clients = RemoteClients::new(store_auth(&envs(&pairs)));
            let endpoint = Arc::from(vt_str::format!("http://{address}/projects/test").as_str());

            let first = upload_to(&clients, &endpoint).await.unwrap_err();
            assert!(matches!(first, UploadError::Unauthorized(_)), "{first:?}");
            assert_eq!(messages(&first), expected);
            assert_eq!(server.join().unwrap().len(), 1);

            // The server is gone, so a request would be a network error.
            for _ in 0..2 {
                let skipped = upload_to(&clients, &endpoint).await.unwrap_err();
                assert!(matches!(skipped, UploadError::Unauthorized(_)), "{skipped:?}");
                assert_eq!(messages(&skipped), messages(&first));
            }
        }
    }

    #[tokio::test]
    async fn other_upload_failures_do_not_stop_uploads() {
        let (address, server) = serve_each(vec![
            b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        ]);
        let clients = RemoteClients::new(StoreAuth::GithubActionsWithoutOidc);
        let endpoint = Arc::from(vt_str::format!("http://{address}/projects/test").as_str());

        let failed = upload_to(&clients, &endpoint).await.unwrap_err();
        assert!(matches!(failed, UploadError::Remote(_)), "{failed:?}");
        upload_to(&clients, &endpoint).await.unwrap();
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cancelling_stops_an_upload() {
        let (endpoint, requested) = serve_stalled(b"");
        let cancel_token = CancellationToken::new();
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        let value = CacheEntryValue { output_archive: None, ..cache_value() };
        let cache_dir = vt_path::current_dir().unwrap();

        let clients = RemoteClients::default();
        let upload =
            clients.upload(&endpoint, &key, &execution_key, &value, &cache_dir, &cancel_token);
        let (uploaded, ()) = tokio::join!(upload, async {
            requested.await.unwrap();
            cancel_token.cancel();
        });
        assert!(matches!(uploaded, Err(UploadError::Cancelled)), "{uploaded:?}");
    }
}

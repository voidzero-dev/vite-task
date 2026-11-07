//! The remote cache tier. After a local miss, the entry is fetched from the
//! remote cache, and after a local update in `read-write` mode, the entry is
//! uploaded to it. Entries are opaque bytes to the remote cache:
//!
//! | Field           | Contents                              |
//! | --------------- | ------------------------------------- |
//! | `key`           | Header + wincode(`CacheEntryKey`)     |
//! | `secondary_key` | Header + wincode(`ExecutionCacheKey`) |
//! | `value`         | zstd(wincode(`CacheEntryValue`))      |
//! | blob            | The `.tar.zst` output archive         |
//!
//! Both keys start with a header containing the cache schema version, the
//! remote value format, and the target OS and architecture. Platforms share an
//! endpoint's namespace, but their keys differ.

use std::{
    fs::File,
    io::{self, Read as _, Write as _},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use vt_path::AbsolutePath;
use vt_plan::{
    cache_metadata::ExecutionCacheKey,
    remote_cache::{RemoteCacheAuth, ResolvedRemoteCacheConfig},
};
use vt_remote_cache::{
    Client, Download, Entry, Fetched,
    auth::{Anonymous, Auth, GithubOidc},
};
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
    #[error("failed to encode the cache entry")]
    Encode(#[from] WriteError),
    #[error("failed to compress the cache entry")]
    Compress(#[source] io::Error),
    /// The entry's value is larger than any client reads back.
    #[error("cache entry is larger than {} MiB", MAX_VALUE_SIZE >> 20)]
    TooLarge,
    /// Ctrl-C cancelled the upload before it finished.
    #[error("interrupted")]
    Interrupted,
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
    CorruptValue(#[source] DecodeValueError),
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

/// Why a fetched value is corrupt.
#[derive(Debug, thiserror::Error)]
pub enum DecodeValueError {
    #[error(transparent)]
    Decompress(io::Error),
    #[error("value decompresses to more than {} MiB", MAX_VALUE_SIZE >> 20)]
    TooLarge,
    #[error(transparent)]
    Decode(wincode::error::ReadError),
}

/// An exact entry that passed validation. It's a hit once its blob, if any,
/// is downloaded.
#[derive(Debug)]
pub(super) struct Restore {
    pub value: CacheEntryValue,
    pub blob_id: Option<Str>,
}

/// Turn the result of a fetch into an entry to restore or a miss. An exact
/// match's entries are checked newest first, and the first that passes is
/// restored. `validate` checks an entry against the current execution. If no
/// entry passes, the miss is the newest entry's: a fingerprint mismatch, or a
/// read failure if it doesn't decode, its blob doesn't match its value, or it
/// can't be validated. A fallback's miss reason compares its key with
/// `cache_key`, and no match is a miss without an entry. A failed fetch is a
/// read failure.
#[expect(
    clippy::result_large_err,
    reason = "`CacheMiss` is intentionally large, and a lookup returns it once"
)]
pub(super) fn resolve(
    fetched: Result<Option<Fetched>, ReadError>,
    cache_key: &CacheEntryKey,
    mut validate: impl FnMut(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>>,
) -> Result<Restore, CacheMiss> {
    let entries = match fetched.map_err(ReadError::into_miss)? {
        Some(Fetched::Exact { entries }) => entries,
        Some(Fetched::Fallback { key }) => {
            let key = decode_key(&key).map_err(ReadError::into_miss)?;
            return Err(CacheMiss::FingerprintMismatch(key.into_mismatch(cache_key)));
        }
        None => return Err(CacheMiss::NotFound),
    };
    let mut newest_miss = None;
    for entry in entries {
        match check_entry(entry, &mut validate) {
            Ok(restore) => return Ok(restore),
            Err(miss) => {
                newest_miss.get_or_insert(miss);
            }
        }
    }
    // An exact match has at least one entry, so there is a newest miss.
    Err(newest_miss.unwrap_or(CacheMiss::NotFound))
}

/// Check one entry of an exact match, as [`resolve`] describes.
#[expect(
    clippy::result_large_err,
    reason = "`CacheMiss` is intentionally large, and a lookup checks only a few entries"
)]
fn check_entry(
    Entry { value, blob_id }: Entry,
    validate: &mut impl FnMut(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>>,
) -> Result<Restore, CacheMiss> {
    let value = decode_value(&value).map_err(|err| ReadError::CorruptValue(err).into_miss())?;
    if value.output_archive.is_some() != blob_id.is_some() {
        return Err(ReadError::MismatchedBlob.into_miss());
    }
    match validate(&value) {
        Ok(None) => Ok(Restore { value, blob_id }),
        Ok(Some(mismatch)) => Err(CacheMiss::FingerprintMismatch(mismatch)),
        Err(err) => Err(ReadError::Validate(err).into_miss()),
    }
}

/// An endpoint and the auth that its requests use. Each has its own client.
#[derive(Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    url: Arc<str>,
    auth: RemoteCacheAuth,
}

/// Remote cache clients, each created when its endpoint is first used with
/// its auth.
#[derive(Debug, Default)]
pub struct RemoteClients {
    clients: Mutex<FxHashMap<ClientKey, Arc<Client>>>,
}

impl RemoteClients {
    fn client(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
    ) -> Result<Arc<Client>, vt_remote_cache::Error> {
        let key =
            ClientKey { url: Arc::clone(&remote_config.url), auth: remote_config.auth.clone() };
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(client) = clients.get(&key) {
            return Ok(Arc::clone(client));
        }
        let client = Arc::new(Client::new(&remote_config.url, build_auth(&remote_config.auth))?);
        clients.insert(key, Arc::clone(&client));
        drop(clients);
        Ok(client)
    }

    /// Fetch the entries stored under `cache_key` in the remote cache that
    /// `remote_config` configures, falling back to the key last stored for
    /// `execution_cache_key`. Returns `None` if neither key matched. Stops
    /// when `cancel_token` is cancelled.
    pub(super) async fn fetch(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cancel_token: &CancellationToken,
    ) -> Result<Option<Fetched>, ReadError> {
        let client = self.client(remote_config).map_err(ReadError::Fetch)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        cancel_token
            .run_until_cancelled(client.fetch(&key, &secondary_key))
            .await
            .ok_or(ReadError::Cancelled)?
            .map_err(ReadError::Fetch)
    }

    /// Download the blob `blob_id` from the remote cache that `remote_config`
    /// configures into `cache_dir`, checking that it decodes as an output
    /// archive as it arrives. It's downloaded to a `.tmp` file, which is
    /// renamed once the check passes and removed otherwise, such as when
    /// `cancel_token` is cancelled. Returns the archive's file name.
    pub(super) async fn download_archive(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
        blob_id: &str,
        cache_dir: &AbsolutePath,
        cancel_token: &CancellationToken,
    ) -> Result<Str, ReadError> {
        let client = self.client(remote_config).map_err(ReadError::Download)?;
        let archive_name = vt_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
        let archive_path = cache_dir.join(archive_name.as_str());
        let temp_path = cache_dir.join(vt_str::format!("{archive_name}.tmp").as_str());
        let result =
            download_checked(&client, blob_id, &temp_path, cancel_token).await.and_then(|()| {
                std::fs::rename(temp_path.as_path(), archive_path.as_path())
                    .map_err(ReadError::WriteArchive)
            });
        if result.is_err() {
            // Best-effort cleanup: the file may not have been created.
            let _ = std::fs::remove_file(temp_path.as_path());
        }
        result.map(|()| archive_name)
    }

    /// Prepare to upload an entry that was just recorded locally, along with
    /// its output archive in `cache_dir`, to the remote cache that
    /// `remote_config` configures: get its client and encode the entry.
    /// Nothing is sent until the returned future is polled, so an invalid
    /// endpoint or an entry that doesn't encode fails here. The future owns
    /// everything the upload needs, so it can run in a spawned task.
    pub(super) fn prepare_upload(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cache_value: &CacheEntryValue,
        cache_dir: &AbsolutePath,
    ) -> Result<impl Future<Output = Result<(), UploadError>> + Send + use<>, UploadError> {
        let client = self.client(remote_config)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        let value = encode_value(cache_value)?;
        let archive = cache_value.output_archive.as_ref().map(|name| cache_dir.join(name.as_str()));
        Ok(async move {
            client.store(&key, &secondary_key, &value, archive.as_deref()).await?;
            Ok(())
        })
    }
}

/// The credentials that requests carry for `auth`.
fn build_auth(auth: &RemoteCacheAuth) -> Arc<dyn Auth> {
    match auth {
        RemoteCacheAuth::Anonymous => Arc::new(Anonymous),
        RemoteCacheAuth::GithubOidc(github_oidc) => Arc::new(GithubOidc::new(
            &github_oidc.request_url,
            github_oidc.request_token.expose(),
            &github_oidc.audience,
        )),
    }
}

/// Uploads running in the background. Each keeps running after its task
/// finishes, until [`Self::wait`] waits for all of them.
#[derive(Debug, Default)]
pub(super) struct RemoteUploads {
    tracker: TaskTracker,
    /// Cancelled when the wait is interrupted, which stops the uploads. It
    /// stays cancelled, since the run ends after Ctrl-C.
    cancel: CancellationToken,
}

impl RemoteUploads {
    /// Send `upload` in the background. If it fails or is cancelled, the
    /// error is set in `error`.
    pub(super) fn spawn(
        &self,
        upload: impl Future<Output = Result<(), UploadError>> + Send + 'static,
        error: Arc<OnceLock<UploadError>>,
    ) {
        let cancel = self.cancel.clone();
        self.tracker.spawn(async move {
            // A cancelled upload sets its error itself, instead of being
            // aborted, so every cancelled upload has one.
            let result =
                cancel.run_until_cancelled(upload).await.unwrap_or(Err(UploadError::Interrupted));
            if let Err(err) = result {
                tracing::debug!(?err, "remote cache upload failed");
                let _ = error.set(err);
            }
        });
    }

    /// The number of uploads still running.
    pub(super) fn pending(&self) -> usize {
        self.tracker.len()
    }

    /// Wait for all uploads to finish. If `interrupt_token` is cancelled
    /// first, or already was, cancel them and wait for them to stop.
    pub(super) async fn wait(&self, interrupt_token: &CancellationToken) {
        self.tracker.close();
        tokio::select! {
            biased;
            () = self.tracker.wait() => {}
            () = interrupt_token.cancelled() => {
                self.cancel.cancel();
                self.tracker.wait().await;
            }
        }
        self.tracker.reopen();
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

/// The zstd level values are compressed at: the default, as for output
/// archives. A value is compressed when its task finishes, and this level
/// shrinks even the largest about 7x in milliseconds. Higher levels shrink
/// values only about a fifth more, at several times the cost.
const VALUE_COMPRESSION_LEVEL: i32 = zstd::DEFAULT_COMPRESSION_LEVEL;

/// The most bytes a value may take before compression. Values are at most a
/// few MB, so this only stops a value from a broken or hostile server, such as
/// a zstd bomb, from exhausting memory. Larger values aren't uploaded, since
/// no client would read them back.
const MAX_VALUE_SIZE: u64 = 32 << 20;

/// Encode a value as it's stored in the remote cache.
fn encode_value(value: &CacheEntryValue) -> Result<Vec<u8>, UploadError> {
    let serialized = serialize_cache(value)?;
    if u64::try_from(serialized.len()).map_or(true, |len| len > MAX_VALUE_SIZE) {
        return Err(UploadError::TooLarge);
    }
    zstd::bulk::compress(&serialized, VALUE_COMPRESSION_LEVEL).map_err(UploadError::Compress)
}

/// Decode a value stored in the remote cache. It's decompressed through a
/// reader that stops after [`MAX_VALUE_SIZE`] bytes, whatever size its frame
/// declares.
fn decode_value(bytes: &[u8]) -> Result<CacheEntryValue, DecodeValueError> {
    let decoder = zstd::Decoder::with_buffer(bytes).map_err(DecodeValueError::Decompress)?;
    let mut reader = decoder.take(MAX_VALUE_SIZE + 1);
    let mut decompressed = Vec::new();
    reader.read_to_end(&mut decompressed).map_err(DecodeValueError::Decompress)?;
    if reader.limit() == 0 {
        return Err(DecodeValueError::TooLarge);
    }
    deserialize_cache(&decompressed).map_err(DecodeValueError::Decode)
}

/// Bump this whenever the remote value encoding changes incompatibly. Both
/// keys carry it, so clients that encode values differently never share
/// entries, and never push out each other's.
const REMOTE_VALUE_FORMAT: u32 = 1;

#[derive(SchemaWrite)]
struct KeyHeader {
    cache_schema_version: u32,
    remote_value_format: u32,
    os: Str,
    arch: Str,
}

/// The key header for this build.
fn encode_header() -> WriteResult<Vec<u8>> {
    serialize_cache(&KeyHeader {
        cache_schema_version: CACHE_SCHEMA_VERSION,
        remote_value_format: REMOTE_VALUE_FORMAT,
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
    use std::{collections::BTreeMap, net::TcpListener, time::Duration};

    use tokio::sync::oneshot;
    use vt_glob::path::PathGlobSet;
    use vt_graph::config::ResolvedGlobConfig;
    use vt_path::{AbsolutePathBuf, RelativePathBuf};
    use vt_plan::{
        cache_metadata::{EnvValueHash, SpawnFingerprint},
        remote_cache::{GithubOidcAuth, RemoteCacheAccess, Secret},
    };

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

    /// An entry with `value`, encoded as uploads encode it, and the blob
    /// `blob_id`.
    fn entry(value: &CacheEntryValue, blob_id: &str) -> Entry {
        Entry { value: encode_value(value).unwrap(), blob_id: Some(Str::from(blob_id)) }
    }

    fn exact(value: &CacheEntryValue) -> Fetched {
        Fetched::Exact { entries: vec![entry(value, "1")] }
    }

    /// `cache_value()` with `inputs` as its globbed inputs, each with hash 1.
    fn with_inputs(inputs: &[&str]) -> CacheEntryValue {
        let globbed_inputs =
            inputs.iter().map(|path| (RelativePathBuf::new(*path).unwrap(), 1)).collect();
        CacheEntryValue { globbed_inputs, ..cache_value() }
    }

    fn inputs(paths: &[&str]) -> BTreeMap<RelativePathBuf, u64> {
        with_inputs(paths).globbed_inputs
    }

    /// The path of an added input, which `miss` must be.
    fn added_input(miss: &CacheMiss) -> &str {
        match miss {
            CacheMiss::FingerprintMismatch(FingerprintMismatch::InputChanged {
                kind: InputChangeKind::Added,
                path,
            }) => path.as_str(),
            miss => panic!("expected an added input, got {miss:?}"),
        }
    }

    /// Validate as `try_hit_remote` does, with no envs and `globbed_inputs` as
    /// the current inputs.
    fn validate_against(
        globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    ) -> impl FnMut(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>> {
        move |value| {
            let workspace_root = vt_path::current_dir().unwrap();
            value.validate(
                &FxHashMap::default(),
                &globbed_inputs,
                &PathGlobSet::default(),
                &workspace_root,
            )
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

    /// The read failure of an exact match whose only entry is `value`, without
    /// a blob. It must be "remote cache value is corrupt".
    fn corrupt_value(value: Vec<u8>) -> Arc<ReadError> {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let fetched = Ok(Some(Fetched::Exact { entries: vec![Entry { value, blob_id: None }] }));
        match resolve(fetched, &key, not_validated).unwrap_err() {
            CacheMiss::RemoteReadFailed(error) => {
                assert_eq!(vt_str::format!("{error}"), "remote cache value is corrupt");
                error
            }
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
    fn newest_entry_that_validates_is_restored() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let entries = vec![entry(&cache_value(), "2"), entry(&cache_value(), "1")];
        let restore =
            resolve(Ok(Some(Fetched::Exact { entries })), &key, validate_against(BTreeMap::new()))
                .unwrap();
        assert_eq!(restore.blob_id.as_deref(), Some("2"));
    }

    #[test]
    fn older_entry_that_validates_is_restored_after_newer_ones_fail() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let entries = vec![
            entry(&with_inputs(&["src/c.txt"]), "4"),
            Entry { value: b"not a cache value".to_vec(), blob_id: Some(Str::from("3")) },
            entry(&with_inputs(&["src/a.txt"]), "2"),
            entry(&with_inputs(&["src/a.txt"]), "1"),
        ];
        let mut validated = 0;
        let mut validate = validate_against(inputs(&["src/a.txt"]));
        let restore = resolve(Ok(Some(Fetched::Exact { entries })), &key, |value| {
            validated += 1;
            validate(value)
        })
        .unwrap();
        assert_eq!(restore.blob_id.as_deref(), Some("2"));
        // The corrupt entry isn't validated, and the entry after the restored
        // one isn't checked.
        assert_eq!(validated, 2);
    }

    #[test]
    fn newest_entry_explains_the_miss_when_none_validates() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let entries = vec![entry(&cache_value(), "2"), entry(&with_inputs(&["src/b.txt"]), "1")];
        let miss = resolve(
            Ok(Some(Fetched::Exact { entries })),
            &key,
            validate_against(inputs(&["src/a.txt"])),
        )
        .unwrap_err();
        assert_eq!(added_input(&miss), "src/a.txt");

        let entries = vec![
            Entry { value: b"not a cache value".to_vec(), blob_id: None },
            entry(&cache_value(), "1"),
        ];
        let miss = resolve(
            Ok(Some(Fetched::Exact { entries })),
            &key,
            validate_against(inputs(&["src/a.txt"])),
        )
        .unwrap_err();
        assert_eq!(read_failure(miss), "remote cache value is corrupt");
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
    fn uploaded_value_round_trips() {
        let value = cache_value();
        let encoded = encode_value(&value).unwrap();
        let serialized = serialize_cache(&value).unwrap();
        assert_eq!(zstd::decode_all(encoded.as_slice()).unwrap(), serialized);
        assert_eq!(serialize_cache(&decode_value(&encoded).unwrap()).unwrap(), serialized);
    }

    #[test]
    fn value_that_is_not_zstd_is_corrupt() {
        let uncompressed = serialize_cache(&cache_value()).unwrap();
        let encoded = encode_value(&cache_value()).unwrap();
        let truncated = encoded[..encoded.len() - 1].to_vec();
        let trailing = [encoded.as_slice(), b"\0"].concat();
        for value in [b"not a cache value".to_vec(), uncompressed, truncated, trailing] {
            let error = corrupt_value(value);
            assert!(
                matches!(*error, ReadError::CorruptValue(DecodeValueError::Decompress(_))),
                "{error:?}"
            );
        }
    }

    #[test]
    fn value_past_the_limit_is_not_uploaded_and_reads_as_corrupt() {
        let content = vec![b'a'; usize::try_from(MAX_VALUE_SIZE).unwrap()];
        let value = CacheEntryValue {
            std_outputs: Arc::new([StdOutput { kind: OutputKind::StdOut, content }]),
            output_archive: None,
            ..cache_value()
        };
        assert!(matches!(encode_value(&value), Err(UploadError::TooLarge)));
        // As a broken server could send it. It would decode if it weren't too
        // large.
        let compressed =
            zstd::bulk::compress(&serialize_cache(&value).unwrap(), VALUE_COMPRESSION_LEVEL)
                .unwrap();
        let error = corrupt_value(compressed);
        assert!(matches!(*error, ReadError::CorruptValue(DecodeValueError::TooLarge)), "{error:?}");
    }

    #[test]
    fn compressed_bytes_that_are_not_a_cache_value_are_corrupt() {
        let mut trailing = serialize_cache(&cache_value()).unwrap();
        trailing.push(0);
        for bytes in [b"not a cache value".as_slice(), &trailing] {
            let value = zstd::bulk::compress(bytes, VALUE_COMPRESSION_LEVEL).unwrap();
            let error = corrupt_value(value);
            assert!(
                matches!(*error, ReadError::CorruptValue(DecodeValueError::Decode(_))),
                "{error:?}"
            );
        }
    }

    #[test]
    fn blob_that_does_not_match_the_value_is_a_read_failure() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let without_archive = CacheEntryValue { output_archive: None, ..cache_value() };
        for (value, blob_id) in [(cache_value(), None), (without_archive, Some(Str::from("1")))] {
            let value = encode_value(&value).unwrap();
            let fetched = Ok(Some(Fetched::Exact { entries: vec![Entry { value, blob_id }] }));
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache entry's blob doesn't match its value");
        }
    }

    #[test]
    fn fallback_key_that_does_not_decode_is_a_corrupt_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let other_header = |cache_schema_version, remote_value_format| {
            let mut stored_key = serialize_cache(&KeyHeader {
                cache_schema_version,
                remote_value_format,
                os: Str::from(std::env::consts::OS),
                arch: Str::from(std::env::consts::ARCH),
            })
            .unwrap();
            stored_key.extend(serialize_cache(&key).unwrap());
            stored_key
        };
        let other_schema = other_header(CACHE_SCHEMA_VERSION + 1, REMOTE_VALUE_FORMAT);
        let other_format = other_header(CACHE_SCHEMA_VERSION, REMOTE_VALUE_FORMAT + 1);
        let mut garbage = encode_header().unwrap();
        garbage.extend(b"not a cache key");
        for stored_key in [b"not a cache key".to_vec(), other_schema, other_format, garbage] {
            let fetched = Ok(Some(Fetched::Fallback { key: stored_key }));
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache key is corrupt");
        }
    }

    /// A `read-write` remote cache at `url` without credentials.
    fn anonymous(url: &str) -> ResolvedRemoteCacheConfig {
        ResolvedRemoteCacheConfig {
            access: RemoteCacheAccess::ReadWrite,
            url: Arc::from(url),
            auth: RemoteCacheAuth::Anonymous,
        }
    }

    /// Serve one request on a loopback endpoint: once the request head
    /// arrives, write `response`, then send nothing more and keep the
    /// connection open until the client closes it. Returns the remote cache
    /// there and a receiver that resolves once `response` is written.
    fn serve_stalled(
        response: &'static [u8],
    ) -> (ResolvedRemoteCacheConfig, oneshot::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
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
        (remote_config, responded)
    }

    #[tokio::test]
    async fn failed_archive_check_stops_the_download() {
        // The response announces more than it sends, so only the failed check
        // can end the download.
        let (remote_config, _) =
            serve_stalled(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\nnot an archive");
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();

        let error = RemoteClients::default()
            .download_archive(&remote_config, "1", &cache_dir, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, ReadError::CorruptArchive(_)), "{error:?}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancelling_stops_a_fetch() {
        let (remote_config, requested) = serve_stalled(b"");
        let cancel_token = CancellationToken::new();
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));

        let clients = RemoteClients::default();
        let fetch = clients.fetch(&remote_config, &key, &execution_key, &cancel_token);
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
        let (remote_config, responded) =
            serve_stalled(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\n");
        let cancel_token = CancellationToken::new();
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();

        let clients = RemoteClients::default();
        let download = clients.download_archive(&remote_config, "1", &cache_dir, &cancel_token);
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

    /// Prepare to upload an entry without an output archive to the remote
    /// cache that `remote_config` configures.
    fn prepare_upload(
        clients: &RemoteClients,
        remote_config: &ResolvedRemoteCacheConfig,
    ) -> Result<impl Future<Output = Result<(), UploadError>> + use<>, UploadError> {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        let value = CacheEntryValue { output_archive: None, ..cache_value() };
        let cache_dir = vt_path::current_dir().unwrap();
        clients.prepare_upload(remote_config, &key, &execution_key, &value, &cache_dir)
    }

    fn upload_error(error: &OnceLock<UploadError>) -> Option<Str> {
        error.get().map(|error| vt_str::format!("{error}"))
    }

    #[test]
    fn upload_to_an_invalid_endpoint_fails_before_it_starts() {
        let remote_config = anonymous("cache.example/projects/test");
        let Err(error) = prepare_upload(&RemoteClients::default(), &remote_config) else {
            panic!("an invalid endpoint should fail");
        };
        assert!(
            matches!(error, UploadError::Remote(vt_remote_cache::Error::InvalidEndpoint(_))),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn failed_upload_sets_its_error() {
        let (error_status, _) =
            serve_stalled(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n");
        // Nothing can listen on port 0.
        let unreachable = anonymous("http://127.0.0.1:0/projects/test");
        let github_oidc_unavailable = ResolvedRemoteCacheConfig {
            auth: RemoteCacheAuth::GithubOidc(GithubOidcAuth {
                request_url: Arc::from("http://127.0.0.1:0/token"),
                request_token: Secret::new(Arc::from("request-token")),
                audience: Arc::from("http://127.0.0.1:0/projects/test"),
            }),
            ..unreachable.clone()
        };
        let clients = RemoteClients::default();
        let uploads = RemoteUploads::default();

        for (remote_config, message) in [
            (error_status, "HTTP status 500"),
            (unreachable, "network error"),
            (github_oidc_unavailable, "failed to authenticate"),
        ] {
            let error = Arc::new(OnceLock::new());
            uploads.spawn(prepare_upload(&clients, &remote_config).unwrap(), Arc::clone(&error));
            uploads.wait(&CancellationToken::new()).await;
            assert_eq!(uploads.pending(), 0);
            assert_eq!(upload_error(&error).as_deref(), Some(message));
        }
    }

    #[tokio::test]
    async fn interrupting_the_wait_cancels_the_uploads() {
        let (remote_config, requested) = serve_stalled(b"");
        let clients = RemoteClients::default();
        let uploads = RemoteUploads::default();
        let error = Arc::new(OnceLock::new());
        uploads.spawn(prepare_upload(&clients, &remote_config).unwrap(), Arc::clone(&error));
        requested.await.unwrap();
        assert_eq!(uploads.pending(), 1);

        let interrupt_token = CancellationToken::new();
        tokio::join!(uploads.wait(&interrupt_token), async { interrupt_token.cancel() });
        assert_eq!(uploads.pending(), 0);
        assert!(matches!(error.get(), Some(UploadError::Interrupted)), "{error:?}");
    }
}

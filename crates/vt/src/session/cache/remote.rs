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

mod retry;

use std::{
    fs::File,
    io::{self, Read as _, Write as _},
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::{Duration, Instant},
};

use bytes::Bytes;
use rustc_hash::FxHashMap;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::Instrument as _;
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

use self::retry::{OutageBreaker, RateLimit, RetryConfig, RetryableError};
use super::{
    BaselineDetails, CACHE_SCHEMA_VERSION, CacheEntryKey, CacheEntryValue, CacheMiss, Checked,
    Comparison, TaskCacheConfig, archive, deserialize_cache, serialize_cache,
};

/// Maximum number of concurrent remote cache uploads in flight per endpoint.
///
/// Bounding uploads per endpoint shapes bursts against backends with
/// per-isolate admission limits, prevents uplink thrashing, and caps open
/// archive file handles.
const MAX_CONCURRENT_STORES: usize = 4;

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
    /// The remote cache endpoint recently failed repeatedly, so this upload
    /// was skipped during the outage cooldown window.
    #[error("remote cache temporarily unavailable")]
    Unavailable,
}

impl RetryableError for UploadError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Remote(err) if err.is_retryable())
    }

    fn is_outage(&self) -> bool {
        matches!(self, Self::Remote(err) if err.is_outage())
    }

    fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Remote(err) if err.is_rate_limited())
    }

    fn is_cancelled(&self) -> bool {
        matches!(self, Self::Interrupted)
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Remote(err) => err.retry_after(),
            Self::Encode(_)
            | Self::Compress(_)
            | Self::TooLarge
            | Self::Interrupted
            | Self::Unavailable => None,
        }
    }

    fn unavailable() -> Self {
        Self::Unavailable
    }
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
    /// The remote cache endpoint recently failed repeatedly, so this read was
    /// skipped during the outage cooldown window.
    #[error("remote cache temporarily unavailable")]
    Unavailable,
}

impl RetryableError for ReadError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Fetch(err) | Self::Download(err) if err.is_retryable())
    }

    fn is_outage(&self) -> bool {
        matches!(self, Self::Fetch(err) | Self::Download(err) if err.is_outage())
    }

    fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Fetch(err) | Self::Download(err) if err.is_rate_limited())
    }

    fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Fetch(err) | Self::Download(err) => err.retry_after(),
            Self::CorruptValue(_)
            | Self::CorruptKey(_)
            | Self::MismatchedBlob
            | Self::CorruptArchive(_)
            | Self::WriteArchive(_)
            | Self::Validate(_)
            | Self::Encode(_)
            | Self::Cancelled
            | Self::Unavailable => None,
        }
    }

    fn unavailable() -> Self {
        Self::Unavailable
    }
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
    /// With `vp run -vv`, what the entry depends on.
    pub baseline: Option<Arc<BaselineDetails>>,
}

/// Turn the result of a fetch into an entry to restore or a miss. An exact
/// match's entries are checked newest first, and the first that passes is
/// restored. `validate` checks an entry against the current execution, and
/// `describe` describes it for `vp run -vv`. If no entry passes, the miss is
/// the newest entry's: the changes it found, or a read failure if it doesn't
/// decode, its blob doesn't match its value, or it can't be validated. A
/// fallback's miss reason compares its key with `cache_key`, and no match is
/// a miss without an entry. A failed fetch is a read failure.
pub(super) fn resolve(
    fetched: Result<Option<Fetched>, ReadError>,
    cache_key: &CacheEntryKey,
    mut validate: impl FnMut(&CacheEntryValue) -> anyhow::Result<Checked>,
    mut describe: impl FnMut(&CacheEntryValue) -> Option<Arc<BaselineDetails>>,
) -> Result<Restore, CacheMiss> {
    let entries = match fetched.map_err(ReadError::into_miss)? {
        Some(Fetched::Exact { entries }) => entries,
        Some(Fetched::Fallback { key }) => {
            let key = decode_key(&key).map_err(ReadError::into_miss)?;
            return Err(CacheMiss::Changed(Comparison::of_key(&key, cache_key)));
        }
        None => return Err(CacheMiss::NotFound),
    };
    let mut newest_miss = None;
    for (index, entry) in entries.into_iter().enumerate() {
        let start = Instant::now();
        let value_bytes = entry.value.len();
        let checked = check_entry(entry, &mut validate, &mut describe);
        tracing::debug!(
            index,
            value_bytes,
            valid = checked.is_ok(),
            elapsed = ?start.elapsed(),
            "checked a remote cache entry"
        );
        match checked {
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
fn check_entry(
    Entry { value, blob_id }: Entry,
    validate: &mut impl FnMut(&CacheEntryValue) -> anyhow::Result<Checked>,
    describe: &mut impl FnMut(&CacheEntryValue) -> Option<Arc<BaselineDetails>>,
) -> Result<Restore, CacheMiss> {
    let value = decode_value(&value).map_err(|err| ReadError::CorruptValue(err).into_miss())?;
    if value.output_archive.is_some() != blob_id.is_some() {
        return Err(ReadError::MismatchedBlob.into_miss());
    }
    match validate(&value) {
        Ok(checked) if checked.changes.is_empty() => {
            let baseline = describe(&value);
            Ok(Restore { value, blob_id, baseline })
        }
        Ok(checked) => {
            let baseline = describe(&value);
            Err(CacheMiss::Changed(Comparison::of_value(&value, checked).with_baseline(baseline)))
        }
        Err(err) => Err(ReadError::Validate(err).into_miss()),
    }
}

/// An endpoint and the auth that its requests use. Each has its own client.
#[derive(Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    url: Arc<str>,
    auth: RemoteCacheAuth,
}

/// Per-endpoint client and session resilience state (upload concurrency cap,
/// `429` cooldown gates, and read/store outage circuit breakers).
#[derive(Debug)]
struct RemoteEndpoint {
    client: Client,
    /// Bounds concurrent uploads in flight for this endpoint.
    store_permits: Semaphore,
    /// Shared cooldown for `429 Too Many Requests` on read operations so
    /// upload rate limits do not stall fetches on the task critical path.
    read_rate_limit: RateLimit,
    /// Shared cooldown for `429 Too Many Requests` on store operations.
    store_rate_limit: RateLimit,
    /// Circuit breaker that short-circuits read requests (fetch and download)
    /// during sustained outages. Kept separate from stores so store failures
    /// do not block the task-critical read path.
    read_outage: OutageBreaker,
    /// Circuit breaker that short-circuits store requests during sustained
    /// outages. Kept separate from reads so successful reads cannot reset
    /// consecutive store failures.
    store_outage: OutageBreaker,
    retry: RetryConfig,
}

impl RemoteEndpoint {
    fn new(
        remote_config: &ResolvedRemoteCacheConfig,
        retry: RetryConfig,
    ) -> Result<Self, vt_remote_cache::Error> {
        let client = Client::new(&remote_config.url, build_auth(&remote_config.auth))?;
        Ok(Self {
            client,
            store_permits: Semaphore::new(MAX_CONCURRENT_STORES),
            read_rate_limit: RateLimit::default(),
            store_rate_limit: RateLimit::default(),
            read_outage: OutageBreaker::new(retry.outage_threshold, retry.outage_cooldown),
            store_outage: OutageBreaker::new(retry.outage_threshold, retry.outage_cooldown),
            retry,
        })
    }

    async fn fetch(
        &self,
        key: &[u8],
        secondary_key: &[u8],
        cancel_token: &CancellationToken,
    ) -> Result<Option<Fetched>, ReadError> {
        cancel_token
            .run_until_cancelled(retry::run(
                "fetch",
                self.retry.read,
                &self.read_rate_limit,
                &self.read_outage,
                || async { self.client.fetch(key, secondary_key).await.map_err(ReadError::Fetch) },
            ))
            .await
            .ok_or(ReadError::Cancelled)?
    }

    async fn download_checked(
        &self,
        blob_id: &str,
        path: &AbsolutePath,
        cancel_token: &CancellationToken,
    ) -> Result<(), ReadError> {
        cancel_token
            .run_until_cancelled(retry::run(
                "download",
                self.retry.read,
                &self.read_rate_limit,
                &self.read_outage,
                || download_once(&self.client, blob_id, path, cancel_token),
            ))
            .await
            .ok_or(ReadError::Cancelled)?
    }

    async fn store(
        &self,
        key: &[u8],
        secondary_key: &[u8],
        value: &[u8],
        blob: Option<&AbsolutePath>,
    ) -> Result<(), UploadError> {
        let queue_start = Instant::now();
        let permit = self.store_permits.acquire().await.expect("store semaphore is never closed");
        let queued = queue_start.elapsed();
        if !queued.is_zero() {
            tracing::debug!(?queued, "acquired remote cache upload slot");
        }
        let result = retry::run(
            "store",
            self.retry.store,
            &self.store_rate_limit,
            &self.store_outage,
            || async {
                self.client
                    .store(key, secondary_key, value, blob)
                    .await
                    .map_err(UploadError::Remote)
            },
        )
        .await;
        drop(permit);
        result
    }
}

/// Remote cache clients, each created when its endpoint is first used with
/// its auth.
#[derive(Debug)]
pub struct RemoteClients {
    clients: Mutex<FxHashMap<ClientKey, Arc<RemoteEndpoint>>>,
    retry: RetryConfig,
}

impl Default for RemoteClients {
    fn default() -> Self {
        Self::with_retry(RetryConfig::DEFAULT)
    }
}

impl RemoteClients {
    fn with_retry(retry: RetryConfig) -> Self {
        Self { clients: Mutex::new(FxHashMap::default()), retry }
    }

    fn endpoint(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
    ) -> Result<Arc<RemoteEndpoint>, vt_remote_cache::Error> {
        let key =
            ClientKey { url: Arc::clone(&remote_config.url), auth: remote_config.auth.clone() };
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(endpoint) = clients.get(&key) {
            return Ok(Arc::clone(endpoint));
        }
        let endpoint = Arc::new(RemoteEndpoint::new(remote_config, self.retry)?);
        clients.insert(key, Arc::clone(&endpoint));
        drop(clients);
        Ok(endpoint)
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
        let endpoint = self.endpoint(remote_config).map_err(ReadError::Fetch)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        endpoint.fetch(&key, &secondary_key, cancel_token).await
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
        let start = Instant::now();
        let endpoint = self.endpoint(remote_config).map_err(ReadError::Download)?;
        let archive_name = vt_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
        let archive_path = cache_dir.join(archive_name.as_str());
        let temp_path = cache_dir.join(vt_str::format!("{archive_name}.tmp").as_str());
        let result =
            endpoint.download_checked(blob_id, &temp_path, cancel_token).await.and_then(|()| {
                std::fs::rename(temp_path.as_path(), archive_path.as_path())
                    .map_err(ReadError::WriteArchive)
            });
        if result.is_err() {
            // Best-effort cleanup: the file may not have been created.
            let _ = std::fs::remove_file(temp_path.as_path());
        }
        tracing::debug!(
            ok = result.is_ok(),
            bytes = result.as_ref().ok().and_then(|&()| archive::file_size(&archive_path)),
            elapsed = ?start.elapsed(),
            "downloaded an output archive"
        );
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
        let start = Instant::now();
        let endpoint = self.endpoint(remote_config)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        let value = encode_value(cache_value)?;
        let archive = cache_value.output_archive.as_ref().map(|name| cache_dir.join(name.as_str()));
        tracing::debug!(
            value_bytes = value.len(),
            archive = archive.is_some(),
            elapsed = ?start.elapsed(),
            "prepared a remote cache upload"
        );
        Ok(async move { endpoint.store(&key, &secondary_key, &value, archive.as_deref()).await })
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
        // The upload outlives its task, so it carries the task's span.
        self.tracker.spawn(
            async move {
                let start = Instant::now();
                // A cancelled upload sets its error itself, instead of being
                // aborted, so every cancelled upload has one.
                let result = cancel
                    .run_until_cancelled(upload)
                    .await
                    .unwrap_or(Err(UploadError::Interrupted));
                match result {
                    Ok(()) => {
                        tracing::debug!(elapsed = ?start.elapsed(), "remote cache upload finished");
                    }
                    Err(err) => {
                        tracing::debug!(?err, elapsed = ?start.elapsed(), "remote cache upload failed");
                        let _ = error.set(err);
                    }
                }
            }
            .in_current_span(),
        );
    }

    /// The number of uploads still running.
    pub(super) fn pending(&self) -> usize {
        self.tracker.len()
    }

    /// Wait for all uploads to finish. If `interrupt_token` is cancelled
    /// first, or already was, cancel them and wait for them to stop.
    pub(super) async fn wait(&self, interrupt_token: &CancellationToken) {
        let start = Instant::now();
        let pending = self.tracker.len();
        self.tracker.close();
        tokio::select! {
            biased;
            () = self.tracker.wait() => {}
            () = interrupt_token.cancelled() => {
                self.cancel.cancel();
                self.tracker.wait().await;
            }
        }
        if pending > 0 {
            tracing::debug!(
                pending,
                interrupted = interrupt_token.is_cancelled(),
                elapsed = ?start.elapsed(),
                "waited for remote cache uploads"
            );
        }
        self.tracker.reopen();
    }
}

/// Chunks buffered between the download and the archive check.
const CHECK_BUFFER_CHUNKS: usize = 16;

/// Single attempt to download the blob `blob_id` to `path` while streaming it
/// through the archive check on a blocking thread, stopping if `cancel_token`
/// is cancelled. Always waits for the blocking check task to finish so the
/// file handle is closed before the caller removes it or retries.
async fn download_once(
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
        let checked = archive::read_output_archive(&mut reader, |_| {});
        if let Some(err) = reader.write_error {
            return Err(ReadError::WriteArchive(err));
        }
        checked.map_err(ReadError::CorruptArchive)
    });
    // Cancelling drops the sender, which ends the check.
    let sent = cancel_token.run_until_cancelled(send_chunks(download, sender)).await;
    // Wait for the check even after a network error or cancellation, so the
    // file is closed before the caller removes it or retries.
    let checked = check.await.unwrap_or_else(|err| Err(ReadError::CorruptArchive(err.into())));
    if cancel_token.is_cancelled() {
        return Err(ReadError::Cancelled);
    }
    match sent.ok_or(ReadError::Cancelled)? {
        Ok(()) => checked,
        Err(err) => {
            // Local disk write failures are not transient network drops.
            if let Err(err @ ReadError::WriteArchive(_)) = checked {
                return Err(err);
            }
            Err(ReadError::Download(err))
        }
    }
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
    use std::{
        collections::BTreeMap,
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use tokio::sync::oneshot;
    use vt_glob::path::PathGlobSet;
    use vt_graph::config::ResolvedGlobConfig;
    use vt_path::{AbsolutePathBuf, RelativePathBuf};
    use vt_plan::{
        cache_metadata::{EnvValueHash, SpawnFingerprint},
        remote_cache::{GithubOidcAuth, RemoteCacheAccess, Secret},
    };

    use super::{retry::RetryPolicy, *};
    use crate::session::{
        cache::{Change, FileCheck, InputChangeKind},
        execute::{
            fingerprint::{PostRunFingerprint, TrackedEnvQuery},
            pipe::{OutputKind, StdOutput},
        },
    };

    /// Zero-delay retry config with production attempt counts so tests
    /// exercise the full retry loop without sleeping.
    const FAST_RETRY: RetryConfig = RetryConfig {
        read: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            ..RetryPolicy::READ
        },
        store: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            ..RetryPolicy::STORE
        },
        ..RetryConfig::DEFAULT
    };

    /// Single-attempt retry config for tests that expect immediate failure.
    const NO_RETRY: RetryConfig = RetryConfig {
        read: RetryPolicy { max_retries: 0, ..FAST_RETRY.read },
        store: RetryPolicy { max_retries: 0, ..FAST_RETRY.store },
        ..FAST_RETRY
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
            CacheMiss::Changed(comparison) => match comparison.changes.as_slice() {
                [Change::File { kind: InputChangeKind::Added, path }] => path.as_str(),
                changes => panic!("expected an added input, got {changes:?}"),
            },
            miss => panic!("expected an added input, got {miss:?}"),
        }
    }

    /// Validate as `try_hit_remote` does, with no envs and `globbed_inputs` as
    /// the current inputs.
    fn validate_against(
        globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    ) -> impl FnMut(&CacheEntryValue) -> anyhow::Result<Checked> {
        move |value| {
            let workspace_root = vt_path::current_dir().unwrap();
            value.validate(
                &FxHashMap::default(),
                &globbed_inputs,
                &PathGlobSet::default(),
                &workspace_root,
                None,
                FileCheck::First,
            )
        }
    }

    const fn no_baseline(_: &CacheEntryValue) -> Option<Arc<BaselineDetails>> {
        None
    }

    fn not_validated(_: &CacheEntryValue) -> anyhow::Result<Checked> {
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
        match resolve(fetched, &key, not_validated, no_baseline).unwrap_err() {
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
        let restore = resolve(
            Ok(Some(exact(&cache_value()))),
            &key,
            validate_against(BTreeMap::new()),
            no_baseline,
        )
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
            no_baseline,
        )
        .unwrap_err();
        let CacheMiss::Changed(comparison) = &miss else { panic!("{miss:?}") };
        assert!(
            matches!(
                comparison.changes.as_slice(),
                [Change::File { kind: InputChangeKind::Added, path }] if path.as_str() == "src/a.txt"
            ),
            "{miss:?}"
        );
    }

    #[test]
    fn newest_entry_that_validates_is_restored() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let entries = vec![entry(&cache_value(), "2"), entry(&cache_value(), "1")];
        let restore = resolve(
            Ok(Some(Fetched::Exact { entries })),
            &key,
            validate_against(BTreeMap::new()),
            no_baseline,
        )
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
        let restore = resolve(
            Ok(Some(Fetched::Exact { entries })),
            &key,
            |value| {
                validated += 1;
                validate(value)
            },
            no_baseline,
        )
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
            no_baseline,
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
            no_baseline,
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
            no_baseline,
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
            no_baseline,
        )
        .unwrap_err();
        // Only the keys are compared, so the files may have changed too, and
        // there's no run to take a duration from.
        assert!(
            matches!(
                &miss,
                CacheMiss::Changed(comparison) if comparison.changes == [Change::InputConfig]
                    && comparison.changes_partial
                    && comparison.baseline_duration.is_none()
            ),
            "{miss:?}"
        );
    }

    #[test]
    fn no_match_is_a_miss_without_an_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let miss = resolve(Ok(None), &key, not_validated, no_baseline).unwrap_err();
        assert!(matches!(miss, CacheMiss::NotFound), "{miss:?}");
    }

    #[test]
    fn failed_fetch_is_a_read_failure() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let fetched = Err(ReadError::Fetch(vt_remote_cache::Error::InvalidEndpoint(None)));
        let miss = resolve(fetched, &key, not_validated, no_baseline).unwrap_err();
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
            let miss = resolve(fetched, &key, not_validated, no_baseline).unwrap_err();
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
            let miss = resolve(fetched, &key, not_validated, no_baseline).unwrap_err();
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
            serve_stalled(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n");
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
        let clients = RemoteClients::with_retry(NO_RETRY);
        let uploads = RemoteUploads::default();

        for (remote_config, message) in [
            (error_status, "HTTP status 403"),
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

    /// Read one complete HTTP/1.1 request (head + `Content-Length` body) from
    /// `listener`, write `raw_response`, and return the raw request bytes.
    fn serve_raw_once(listener: &TcpListener, raw_response: &[u8]) -> Vec<u8> {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        let header_end = loop {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request head ended");
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let content_length: usize = std::str::from_utf8(&request[..header_end])
            .unwrap()
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request body ended");
            request.extend_from_slice(&buf[..n]);
        }
        stream.write_all(raw_response).unwrap();
        request
    }

    fn serve_once(listener: &TcpListener, status_line: &str, body: &[u8]) -> Vec<u8> {
        let head = vt_str::format!(
            "{status_line}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        serve_raw_once(listener, &[head.as_bytes(), body].concat())
    }

    fn no_request_waiting(listener: &TcpListener) -> bool {
        listener.set_nonblocking(true).unwrap();
        let none = matches!(listener.accept(), Err(err) if err.kind() == io::ErrorKind::WouldBlock);
        listener.set_nonblocking(false).unwrap();
        none
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    fn build_valid_archive() -> Vec<u8> {
        let payload = vec![b'x'; 4096];
        let mut encoder = zstd::Encoder::new(Vec::new(), zstd::DEFAULT_COMPRESSION_LEVEL).unwrap();
        {
            let mut tar = tar::Builder::new(&mut encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(u64::try_from(payload.len()).unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "dist/out.txt", payload.as_slice()).unwrap();
            tar.finish().unwrap();
        }
        encoder.finish().unwrap()
    }

    #[tokio::test]
    async fn fetch_retries_transient_statuses_and_network_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            // Attempt 1: 503 with Retry-After: 60 (must not stall or fail).
            serve_raw_once(
                &listener,
                b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 60\r\nconnection: close\r\ncontent-length: 39\r\n\r\nService unavailable (concurrency_limit)",
            );
            // Attempt 2: connection dropped mid-response (network error).
            serve_raw_once(&listener, b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n");
            // Attempt 3: 404 miss succeeds.
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"");
        });

        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        let start = Instant::now();
        let fetched = RemoteClients::with_retry(FAST_RETRY)
            .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(fetched, None);
        assert!(start.elapsed() < Duration::from_secs(5), "503 Retry-After: 60 should be ignored");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn download_retries_header_and_mid_stream_network_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();
        let archive_bytes = build_valid_archive();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let truncated_prefix = &archive_bytes[..archive_bytes.len() / 2];
            // Attempt 1: 503 before headers.
            serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"busy");
            // Attempt 2: valid prefix truncated before announced Content-Length -> mid-stream network error.
            serve_raw_once(
                &listener,
                &[
                    vt_str::format!(
                        "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                        archive_bytes.len()
                    )
                    .as_bytes(),
                    truncated_prefix,
                ]
                .concat(),
            );
            // Attempt 3: complete valid archive.
            serve_raw_once(
                &listener,
                &[
                    vt_str::format!(
                        "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                        archive_bytes.len()
                    )
                    .as_bytes(),
                    &archive_bytes,
                ]
                .concat(),
            );
        });

        let archive_name = RemoteClients::with_retry(FAST_RETRY)
            .download_archive(&remote_config, "1", &cache_dir, &CancellationToken::new())
            .await
            .unwrap();
        assert!(cache_dir.join(archive_name.as_str()).as_path().is_file());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn store_retries_transient_errors_and_resends_blob() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = AbsolutePathBuf::new(dir.path().to_path_buf()).unwrap();
        std::fs::write(cache_dir.join("uploader.tar.zst").as_path(), b"archive bytes").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let first = serve_raw_once(
                &listener,
                b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 60\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            // Drop connection before sending headers on attempt 2.
            {
                let (stream, _) = listener.accept().unwrap();
                drop(stream);
            }
            let third = serve_once(&listener, "HTTP/1.1 200 OK", b"");
            (first, third)
        });

        let clients = RemoteClients::with_retry(FAST_RETRY);
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        clients
            .prepare_upload(&remote_config, &key, &execution_key, &cache_value(), &cache_dir)
            .unwrap()
            .await
            .unwrap();
        let (first, third) = server.join().unwrap();
        let blob_part =
            b"name=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\narchive bytes\r\n";
        assert!(contains(&first, blob_part));
        assert!(contains(&third, blob_part));
    }

    #[tokio::test]
    async fn non_retryable_statuses_fail_without_retrying() {
        for status_line in [
            "HTTP/1.1 400 Bad Request",
            "HTTP/1.1 401 Unauthorized",
            "HTTP/1.1 403 Forbidden",
            "HTTP/1.1 413 Payload Too Large",
            "HTTP/1.1 501 Not Implemented",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let remote_config = anonymous(&vt_str::format!(
                "http://{}/projects/test",
                listener.local_addr().unwrap()
            ));
            let server = {
                let listener = listener.try_clone().unwrap();
                std::thread::spawn(move || serve_once(&listener, status_line, b"nope"))
            };

            let clients = RemoteClients::with_retry(FAST_RETRY);
            let error = prepare_upload(&clients, &remote_config).unwrap().await.unwrap_err();
            assert!(matches!(error, UploadError::Remote(_)), "{status_line}: {error:?}");
            server.join().unwrap();
            assert!(no_request_waiting(&listener), "{status_line} should not retry");
        }
    }

    #[tokio::test]
    async fn rate_limit_429_respects_retry_after() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let cooldown = Duration::from_millis(80);
        let retry = RetryConfig {
            read: RetryPolicy { max_retry_after: cooldown, ..FAST_RETRY.read },
            store: RetryPolicy { max_retry_after: cooldown, ..FAST_RETRY.store },
            ..FAST_RETRY
        };
        let server = std::thread::spawn(move || {
            // `Retry-After: 5` is clamped to `cooldown` (80 ms).
            serve_raw_once(
                &listener,
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 5\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            serve_once(&listener, "HTTP/1.1 200 OK", b"");
        });

        let clients = RemoteClients::with_retry(retry);
        let start = Instant::now();
        prepare_upload(&clients, &remote_config).unwrap().await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(60),
            "429 Retry-After cooldown was not respected: {:?}",
            start.elapsed()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rate_limit_429_pauses_same_kind_peers_without_stalling_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let cooldown = Duration::from_millis(80);
        let retry = RetryConfig {
            read: RetryPolicy { max_retries: 0, max_retry_after: cooldown, ..FAST_RETRY.read },
            store: RetryPolicy { max_retries: 0, max_retry_after: cooldown, ..FAST_RETRY.store },
            ..FAST_RETRY
        };
        let server = std::thread::spawn(move || {
            serve_raw_once(
                &listener,
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 5\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"");
            serve_once(&listener, "HTTP/1.1 200 OK", b"");
        });

        let clients = RemoteClients::with_retry(retry);
        let err = prepare_upload(&clients, &remote_config).unwrap().await.unwrap_err();
        assert!(matches!(err, UploadError::Remote(ref r) if r.is_rate_limited()), "{err:?}");

        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));
        let fetch_start = Instant::now();
        assert_eq!(
            clients
                .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
                .await
                .unwrap(),
            None
        );
        assert!(
            fetch_start.elapsed() < Duration::from_millis(50),
            "store 429 should not stall fetch: {:?}",
            fetch_start.elapsed()
        );

        let store_start = Instant::now();
        prepare_upload(&clients, &remote_config).unwrap().await.unwrap();
        assert!(
            store_start.elapsed() >= Duration::from_millis(40),
            "peer store did not wait for shared 429 cooldown: {:?}",
            store_start.elapsed()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn outage_breaker_short_circuits_per_kind_independently() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let retry = RetryConfig {
            outage_threshold: 2,
            outage_cooldown: Duration::from_secs(30),
            ..NO_RETRY
        };
        let server = {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                // Store #1 fails, interleaved fetch succeeds (must not reset store failures),
                // store #2 fails (opens store breaker), fetch still works,
                // then two fetch failures open read breaker.
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
                serve_once(&listener, "HTTP/1.1 404 Not Found", b"");
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
                serve_once(&listener, "HTTP/1.1 404 Not Found", b"");
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
            })
        };

        let clients = RemoteClients::with_retry(retry);
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let execution_key = ExecutionCacheKey::ExecAPI(Arc::from([]));

        let err = prepare_upload(&clients, &remote_config).unwrap().await.unwrap_err();
        assert!(matches!(err, UploadError::Remote(ref r) if r.is_outage()), "{err:?}");
        // A successful fetch must not reset consecutive store failures.
        assert_eq!(
            clients
                .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
                .await
                .unwrap(),
            None
        );
        let err = prepare_upload(&clients, &remote_config).unwrap().await.unwrap_err();
        assert!(matches!(err, UploadError::Remote(ref r) if r.is_outage()), "{err:?}");

        // Store breaker is now open, but reads still reach the server.
        assert_eq!(
            clients
                .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
                .await
                .unwrap(),
            None
        );
        let err = prepare_upload(&clients, &remote_config).unwrap().await.unwrap_err();
        assert!(matches!(err, UploadError::Unavailable), "{err:?}");
        assert_eq!(err.to_string(), "remote cache temporarily unavailable");

        // Two consecutive fetch failures open the read breaker as well.
        for _ in 0..2 {
            let err = clients
                .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
                .await
                .unwrap_err();
            assert!(matches!(err, ReadError::Fetch(ref r) if r.is_outage()), "{err:?}");
        }
        server.join().unwrap();

        let err = clients
            .fetch(&remote_config, &key, &execution_key, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, ReadError::Unavailable), "{err:?}");
        assert_eq!(err.to_string(), "remote cache temporarily unavailable");
        assert!(no_request_waiting(&listener));
    }

    #[tokio::test]
    async fn concurrent_stores_are_capped_at_max_concurrent_stores() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let total_stores = MAX_CONCURRENT_STORES * 2;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let (filled_tx, filled_rx) = std::sync::mpsc::channel::<()>();

        let server = {
            let in_flight = Arc::clone(&in_flight);
            let max_in_flight = Arc::clone(&max_in_flight);
            std::thread::spawn(move || {
                let mut handlers = Vec::with_capacity(total_stores);
                for _ in 0..total_stores {
                    let (mut stream, _) = listener.accept().unwrap();
                    let in_flight = Arc::clone(&in_flight);
                    let max_in_flight = Arc::clone(&max_in_flight);
                    let release_rx = Arc::clone(&release_rx);
                    let filled_tx = filled_tx.clone();
                    handlers.push(std::thread::spawn(move || {
                        let mut request = Vec::new();
                        let mut buf = [0; 4096];
                        let header_end = loop {
                            let n = stream.read(&mut buf).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buf[..n]);
                            if let Some(pos) =
                                request.windows(4).position(|window| window == b"\r\n\r\n")
                            {
                                break pos + 4;
                            }
                        };
                        let content_length: usize = std::str::from_utf8(&request[..header_end])
                            .unwrap()
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        while request.len() < header_end + content_length {
                            let n = stream.read(&mut buf).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buf[..n]);
                        }
                        let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_in_flight.fetch_max(current, Ordering::SeqCst);
                        if current == MAX_CONCURRENT_STORES {
                            let _ = filled_tx.send(());
                        }
                        let _ = release_rx.lock().unwrap().recv();
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 0\r\n\r\n")
                            .unwrap();
                    }));
                }
                for handler in handlers {
                    handler.join().unwrap();
                }
            })
        };

        let clients = RemoteClients::with_retry(FAST_RETRY);
        let mut tasks = Vec::with_capacity(total_stores);
        for _ in 0..total_stores {
            let upload = prepare_upload(&clients, &remote_config).unwrap();
            tasks.push(tokio::spawn(upload));
        }

        tokio::task::spawn_blocking(move || {
            filled_rx.recv().unwrap();
            for _ in 0..total_stores {
                release_tx.send(()).unwrap();
            }
        })
        .await
        .unwrap();

        for task in tasks {
            task.await.unwrap().unwrap();
        }
        server.join().unwrap();
        assert_eq!(max_in_flight.load(Ordering::SeqCst), MAX_CONCURRENT_STORES);
    }

    #[tokio::test]
    async fn interrupting_the_wait_cancels_the_uploads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let remote_config =
            anonymous(&vt_str::format!("http://{}/projects/test", listener.local_addr().unwrap()));
        let total = MAX_CONCURRENT_STORES + 2;
        let (all_slots_filled_tx, all_slots_filled_rx) = oneshot::channel();
        std::thread::spawn(move || {
            // Hold open `MAX_CONCURRENT_STORES` requests while more uploads are
            // pending so cancelling interrupts both active and queued uploads.
            let mut streams = Vec::new();
            for _ in 0..MAX_CONCURRENT_STORES {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                streams.push(stream);
            }
            let _ = all_slots_filled_tx.send(());
            // Wait until the client drops the connections on cancel.
            for mut stream in streams {
                let mut buf = [0; 64];
                while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
            }
        });

        let clients = RemoteClients::default();
        let uploads = RemoteUploads::default();
        let mut errors = Vec::with_capacity(total);
        for _ in 0..total {
            let error = Arc::new(OnceLock::new());
            uploads.spawn(prepare_upload(&clients, &remote_config).unwrap(), Arc::clone(&error));
            errors.push(error);
        }
        all_slots_filled_rx.await.unwrap();
        assert_eq!(uploads.pending(), total);

        let interrupt_token = CancellationToken::new();
        tokio::join!(uploads.wait(&interrupt_token), async { interrupt_token.cancel() });
        assert_eq!(uploads.pending(), 0);
        for error in errors {
            assert!(matches!(error.get(), Some(UploadError::Interrupted)), "{error:?}");
        }
    }
}

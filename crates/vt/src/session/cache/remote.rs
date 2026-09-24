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
    error::Error,
    sync::{Arc, Mutex, PoisonError},
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use vt_path::AbsolutePath;
use vt_plan::cache_metadata::ExecutionCacheKey;
use vt_remote_cache::{Client, Fetched};
use vt_str::Str;
use wincode::{
    SchemaWrite,
    error::{WriteError, WriteResult},
};

use super::{
    CACHE_SCHEMA_VERSION, CacheEntryKey, CacheEntryValue, CacheMiss, FingerprintMismatch,
    TaskCacheConfig, archive, deserialize_cache, serialize_cache,
};

/// Why an entry wasn't uploaded. The message names only the kind of failure,
/// so it's the same on every platform. The details are in the source.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error(transparent)]
    Remote(#[from] vt_remote_cache::Error),
    #[error("failed to encode the cache entry")]
    Encode(#[from] WriteError),
}

impl UploadError {
    pub fn to_failure(&self) -> RemoteCacheFailure {
        RemoteCacheFailure { reason: vt_str::format!("{self}"), details: chain(self.source()) }
    }
}

/// A remote cache failure as reported. The reason is the same on every
/// platform. `--last-details` also shows the details, which come from the
/// underlying errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCacheFailure {
    pub reason: Str,
    pub details: Option<Str>,
}

impl RemoteCacheFailure {
    /// The reason followed by the details, if any.
    pub fn with_details(&self) -> Str {
        self.details.as_ref().map_or_else(
            || self.reason.clone(),
            |details| vt_str::format!("{}: {details}", self.reason),
        )
    }
}

/// The messages of `error` and its sources, joined with `: `.
fn chain(error: Option<&(dyn Error + 'static)>) -> Option<Str> {
    std::iter::successors(error, |&err| err.source()).fold(None, |chain, err| {
        Some(
            chain.map_or_else(
                || vt_str::format!("{err}"),
                |chain| vt_str::format!("{chain}: {err}"),
            ),
        )
    })
}

/// Why no entry could be read from the remote cache. It's a cache miss, and
/// the message is its reason. The message names only the kind of failure, so
/// it's the same on every platform.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("remote cache fetch failed ({0})")]
    Fetch(#[source] vt_remote_cache::Error),
    #[error("remote cache download failed ({0})")]
    Download(#[source] vt_remote_cache::Error),
    #[error("remote cache value is corrupt")]
    CorruptValue(#[source] wincode::error::ReadError),
    #[error("remote cache key is corrupt")]
    CorruptKey(#[source] Option<wincode::error::ReadError>),
    #[error("downloaded archive is corrupt")]
    CorruptArchive(#[source] std::io::Error),
    #[error("remote cache entry couldn't be validated")]
    Validate(#[source] anyhow::Error),
    #[error("failed to encode the cache key")]
    Encode(#[from] WriteError),
}

impl ReadError {
    /// The miss for this failure. The full error is only logged.
    pub(super) fn into_miss(self) -> CacheMiss {
        tracing::debug!(err = ?self, "remote cache read failed");
        CacheMiss::RemoteReadFailed(vt_str::format!("{self}"))
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
/// reason compares its key with `cache_key`. A failed fetch, an entry that
/// doesn't decode, or a validation error is a read failure.
#[expect(
    clippy::result_large_err,
    reason = "`CacheMiss` is intentionally large, and a lookup returns it once"
)]
pub(super) fn resolve(
    fetched: Result<Fetched, ReadError>,
    cache_key: &CacheEntryKey,
    validate: impl FnOnce(&CacheEntryValue) -> anyhow::Result<Option<FingerprintMismatch>>,
) -> Result<Restore, CacheMiss> {
    let (value, blob_id) = match fetched.map_err(ReadError::into_miss)? {
        Fetched::Exact { value, blob_id } => {
            let value = deserialize_cache(&value)
                .map_err(|err| ReadError::CorruptValue(err).into_miss())?;
            (value, blob_id)
        }
        Fetched::Fallback { key } => {
            let key = decode_key(&key).map_err(ReadError::into_miss)?;
            return Err(CacheMiss::FingerprintMismatch(key.into_mismatch(cache_key)));
        }
        Fetched::NotFound => return Err(CacheMiss::NotFound),
    };
    match validate(&value) {
        Ok(None) => Ok(Restore { value, blob_id }),
        Ok(Some(mismatch)) => Err(CacheMiss::FingerprintMismatch(mismatch)),
        Err(err) => Err(ReadError::Validate(err).into_miss()),
    }
}

/// Remote cache clients, each created when its endpoint is first used.
#[derive(Debug, Default)]
pub struct RemoteClients {
    clients: Mutex<FxHashMap<Arc<str>, Arc<Client>>>,
}

impl RemoteClients {
    fn client(&self, endpoint: &Arc<str>) -> Result<Arc<Client>, vt_remote_cache::Error> {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(client) = clients.get(endpoint) {
            return Ok(Arc::clone(client));
        }
        let client = Arc::new(Client::new(endpoint)?);
        clients.insert(Arc::clone(endpoint), Arc::clone(&client));
        drop(clients);
        Ok(client)
    }

    /// Fetch the entry stored under `cache_key`, falling back to the entry
    /// last stored for `execution_cache_key`.
    pub(super) async fn fetch(
        &self,
        endpoint: &Arc<str>,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
    ) -> Result<Fetched, ReadError> {
        let client = self.client(endpoint).map_err(ReadError::Fetch)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        client.fetch(&key, &secondary_key).await.map_err(ReadError::Fetch)
    }

    /// Download the blob `blob_id` into `cache_dir` and check that it decodes
    /// as an output archive. Returns the archive's file name. If either step
    /// fails, the file is removed.
    pub(super) async fn download_archive(
        &self,
        endpoint: &Arc<str>,
        blob_id: &str,
        cache_dir: &AbsolutePath,
    ) -> Result<Str, ReadError> {
        let client = self.client(endpoint).map_err(ReadError::Download)?;
        let archive_name = vt_str::format!("{}.tar.zst", uuid::Uuid::new_v4());
        let archive_path = cache_dir.join(archive_name.as_str());
        let result = match client.download(blob_id, &archive_path).await {
            Ok(()) => {
                archive::check_output_archive(&archive_path).map_err(ReadError::CorruptArchive)
            }
            Err(err) => Err(ReadError::Download(err)),
        };
        if result.is_err() {
            // Best-effort cleanup: the file may not have been created.
            let _ = std::fs::remove_file(archive_path.as_path());
        }
        result.map(|()| archive_name)
    }

    /// Upload an entry that was just recorded locally, along with its output
    /// archive in `cache_dir`.
    pub(super) async fn upload(
        &self,
        endpoint: &Arc<str>,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cache_value: &CacheEntryValue,
        cache_dir: &AbsolutePath,
    ) -> Result<(), UploadError> {
        let client = self.client(endpoint)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        let value = serialize_cache(cache_value)?;
        let archive = cache_value.output_archive.as_ref().map(|name| cache_dir.join(name.as_str()));
        client.store(&key, &secondary_key, &value, archive.as_deref()).await?;
        Ok(())
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
    use std::{collections::BTreeMap, time::Duration};

    use vt_graph::config::ResolvedGlobConfig;
    use vt_path::RelativePathBuf;
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
        panic!("only exact entries that decode are validated")
    }

    fn read_failure(miss: CacheMiss) -> Str {
        match miss {
            CacheMiss::RemoteReadFailed(reason) => reason,
            miss => panic!("expected a read failure, got {miss:?}"),
        }
    }

    #[test]
    fn exact_entry_that_validates_is_restored() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let restore =
            resolve(Ok(exact(&cache_value())), &key, validate_against(BTreeMap::new())).unwrap();
        assert_eq!(restore.blob_id.as_deref(), Some("1"));
        assert_eq!(restore.value.std_outputs[0].content, b"built\n");
        assert_eq!(restore.value.duration, Duration::from_millis(5));
    }

    #[test]
    fn exact_entry_that_fails_validation_is_a_mismatch() {
        let current_inputs = BTreeMap::from([(RelativePathBuf::new("src/a.txt").unwrap(), 1)]);
        let miss = resolve(
            Ok(exact(&cache_value())),
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
            Ok(exact(&value)),
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
            Ok(Fetched::Fallback { key: stored_key }),
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
    fn not_found_is_a_miss_without_an_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let miss = resolve(Ok(Fetched::NotFound), &key, not_validated).unwrap_err();
        assert!(matches!(miss, CacheMiss::NotFound), "{miss:?}");
    }

    #[test]
    fn failed_fetch_is_a_read_failure() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let fetched = Err(ReadError::Fetch(vt_remote_cache::Error::InvalidEndpoint(None)));
        let miss = resolve(fetched, &key, not_validated).unwrap_err();
        assert_eq!(read_failure(miss), "remote cache fetch failed (invalid endpoint)");
    }

    #[test]
    fn value_that_does_not_decode_is_a_corrupt_entry() {
        let key = cache_key(ResolvedGlobConfig::default_auto());
        let mut trailing = serialize_cache(&cache_value()).unwrap();
        trailing.push(0);
        for value in [b"not a cache value".to_vec(), trailing] {
            let fetched = Ok(Fetched::Exact { value, blob_id: None });
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache value is corrupt");
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
            let fetched = Ok(Fetched::Fallback { key: stored_key });
            let miss = resolve(fetched, &key, not_validated).unwrap_err();
            assert_eq!(read_failure(miss), "remote cache key is corrupt");
        }
    }
}

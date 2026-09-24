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
    CACHE_SCHEMA_VERSION, CacheEntryKey, CacheEntryValue, CacheMiss, TaskCacheConfig, archive,
    deserialize_cache, serialize_cache,
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
    #[error("failed to encode the cache key")]
    Encode(#[from] WriteError),
}

impl From<ReadError> for CacheMiss {
    fn from(err: ReadError) -> Self {
        Self::RemoteReadFailed(vt_str::format!("{err}"))
    }
}

/// A decoded fetch response.
#[derive(Debug)]
pub(super) enum RemoteEntry {
    /// The entry stored under the current key. Its blob, if any, isn't
    /// downloaded yet.
    Exact {
        value: CacheEntryValue,
        blob_id: Option<Str>,
    },
    /// The entry last stored for this execution, under a different key.
    Fallback {
        key: CacheEntryKey,
    },
    NotFound,
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
    ) -> Result<RemoteEntry, ReadError> {
        let client = self.client(endpoint).map_err(ReadError::Fetch)?;
        let key = encode_key(cache_key)?;
        let secondary_key = encode_key(execution_cache_key)?;
        let fetched = client.fetch(&key, &secondary_key).await.map_err(ReadError::Fetch)?;
        decode_fetched(fetched)
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

/// Decode the value of an exact response, or the key of a fallback response.
fn decode_fetched(fetched: Fetched) -> Result<RemoteEntry, ReadError> {
    Ok(match fetched {
        Fetched::Exact { value, blob_id } => RemoteEntry::Exact {
            value: deserialize_cache(&value).map_err(ReadError::CorruptValue)?,
            blob_id,
        },
        Fetched::Fallback { key } => RemoteEntry::Fallback { key: decode_key(&key)? },
        Fetched::NotFound => RemoteEntry::NotFound,
    })
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

    use super::*;
    use crate::session::execute::{
        fingerprint::PostRunFingerprint,
        pipe::{OutputKind, StdOutput},
    };

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

    #[test]
    fn exact_response_gives_the_entry_to_restore() {
        let value = serialize_cache(&cache_value()).unwrap();
        let fetched = Fetched::Exact { value, blob_id: Some(Str::from("1")) };
        let RemoteEntry::Exact { value, blob_id } = decode_fetched(fetched).unwrap() else {
            panic!("expected an exact entry");
        };
        assert_eq!(blob_id.as_deref(), Some("1"));
        assert_eq!(value.std_outputs[0].content, b"built\n");
        assert_eq!(value.duration, Duration::from_millis(5));
    }

    #[test]
    fn not_found_response_has_no_entry() {
        assert!(matches!(decode_fetched(Fetched::NotFound), Ok(RemoteEntry::NotFound)));
    }

    fn miss_reason(error: ReadError) -> Str {
        match CacheMiss::from(error) {
            CacheMiss::RemoteReadFailed(reason) => reason,
            miss => panic!("expected a read failure, got {miss:?}"),
        }
    }

    #[test]
    fn value_that_does_not_decode_is_a_corrupt_entry() {
        let mut value = serialize_cache(&cache_value()).unwrap();
        value.push(0);
        for value in [b"not a cache value".to_vec(), value] {
            let error = decode_fetched(Fetched::Exact { value, blob_id: None }).unwrap_err();
            assert!(matches!(error, ReadError::CorruptValue(_)), "{error:?}");
            assert_eq!(miss_reason(error), "remote cache value is corrupt");
        }
    }

    #[test]
    fn fallback_key_that_does_not_decode_is_a_corrupt_entry() {
        let mut other_header = serialize_cache(&KeyHeader {
            cache_schema_version: CACHE_SCHEMA_VERSION + 1,
            os: Str::from(std::env::consts::OS),
            arch: Str::from(std::env::consts::ARCH),
        })
        .unwrap();
        other_header.extend(b"key");
        let mut garbage = encode_header().unwrap();
        garbage.extend(b"not a cache key");
        for key in [b"not a cache key".to_vec(), other_header, garbage] {
            let error = decode_fetched(Fetched::Fallback { key }).unwrap_err();
            assert!(matches!(error, ReadError::CorruptKey(_)), "{error:?}");
            assert_eq!(miss_reason(error), "remote cache key is corrupt");
        }
    }
}

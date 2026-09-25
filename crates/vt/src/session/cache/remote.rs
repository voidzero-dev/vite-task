//! The remote cache tier. After a local update in `read-write` mode, the entry
//! is uploaded to the remote cache as opaque bytes:
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

use std::sync::{Arc, Mutex, PoisonError};

use rustc_hash::FxHashMap;
use vt_path::AbsolutePath;
use vt_plan::cache_metadata::ExecutionCacheKey;
use vt_remote_cache::Client;
use vt_str::Str;
use wincode::{
    SchemaWrite,
    error::{WriteError, WriteResult},
};

use super::{
    CACHE_SCHEMA_VERSION, CacheEntryKey, CacheEntryValue, TaskCacheConfig, serialize_cache,
};

/// Why an entry wasn't uploaded. The message names only the kind of failure,
/// so it's the same on every platform.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error(transparent)]
    Remote(#[from] vt_remote_cache::Error),
    #[error("failed to encode the cache entry")]
    Encode(#[from] WriteError),
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

/// Encode a key with the header for this build.
fn encode_key<K: SchemaWrite<TaskCacheConfig, Src = K>>(key: &K) -> WriteResult<Vec<u8>> {
    let header = KeyHeader {
        cache_schema_version: CACHE_SCHEMA_VERSION,
        os: Str::from(std::env::consts::OS),
        arch: Str::from(std::env::consts::ARCH),
    };
    let mut bytes = serialize_cache(&header)?;
    bytes.extend(serialize_cache(key)?);
    Ok(bytes)
}

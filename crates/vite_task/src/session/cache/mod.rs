//! Execution cache for storing and retrieving cached command results.

pub mod display;

use std::{fmt::Display, fs::File, io::Write, sync::Arc, time::Duration};

use bincode::{Decode, Encode, decode_from_slice, encode_to_vec};
// Re-export display functions for convenience
pub use display::{format_cache_status_inline, format_cache_status_summary};
use rusqlite::{Connection, OptionalExtension as _, config::DbConfig};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_task_plan::cache_metadata::{CacheMetadata, ExecutionCacheKey, SpawnFingerprint};

use super::execute::{
    fingerprint::{PostRunFingerprint, PostRunFingerprintMismatch},
    spawn::StdOutput,
};

/// Command cache value, for validating post-run fingerprint after the spawn fingerprint is matched,
/// and replaying the std outputs if validated.
#[derive(Debug, Encode, Decode, Serialize)]
pub struct CommandCacheValue {
    pub post_run_fingerprint: PostRunFingerprint,
    pub std_outputs: Arc<[StdOutput]>,
    pub duration: Duration,
}

#[derive(Debug)]
pub struct ExecutionCache {
    conn: Mutex<Connection>,
}

const BINCODE_CONFIG: bincode::config::Configuration = bincode::config::standard();

#[derive(Debug, Serialize, Deserialize)]
pub enum CacheMiss {
    NotFound,
    FingerprintMismatch(FingerprintMismatch),
}

#[derive(Debug, Serialize, Deserialize)]
pub enum FingerprintMismatch {
    /// Found the cache entry of the same task run, but the spawn fingerprint mismatches
    /// this happens when the command itself or an env changes.
    SpawnFingerprintMismatch {
        /// The fingerprint from the cached entry
        old: SpawnFingerprint,
        /// The fingerprint of the current execution
        new: SpawnFingerprint,
    },
    /// Found the cache entry with the same spawn fingerprint, but the post-run fingerprint mismatches
    PostRunFingerprintMismatch(PostRunFingerprintMismatch),
}

impl Display for FingerprintMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFingerprintMismatch { old, new } => {
                write!(f, "Spawn fingerprint changed: old={old:?}, new={new:?}")
            }
            Self::PostRunFingerprintMismatch(diff) => Display::fmt(diff, f),
        }
    }
}

impl ExecutionCache {
    pub fn load_from_path(cache_path: AbsolutePathBuf) -> anyhow::Result<Self> {
        let path: &AbsolutePath = cache_path.as_ref();
        tracing::info!("Creating task cache directory at {:?}", path);
        std::fs::create_dir_all(path)?;

        // Use file lock to prevent race conditions when multiple processes initialize the database
        let lock_path = path.join("db_open.lock");
        let lock_file = File::create(lock_path.as_path())?;
        lock_file.lock()?;

        let db_path = path.join("cache.db");
        let conn = Connection::open(db_path.as_path())?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        loop {
            let user_version: u32 = conn.query_one("PRAGMA user_version", (), |row| row.get(0))?;
            match user_version {
                0 => {
                    // fresh new db
                    conn.execute(
                        "CREATE TABLE spawn_fingerprint_cache (key BLOB PRIMARY KEY, value BLOB);",
                        (),
                    )?;
                    conn.execute(
                        "CREATE TABLE execution_key_to_fingerprint (key BLOB PRIMARY KEY, value BLOB);",
                        (),
                    )?;
                    conn.execute("PRAGMA user_version = 6", ())?;
                }
                1..=5 => {
                    // old internal db version. reset
                    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_RESET_DATABASE, true)?;
                    conn.execute("VACUUM", ())?;
                    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_RESET_DATABASE, false)?;
                }
                6 => break, // current version
                6.. => {
                    return Err(anyhow::anyhow!("Unrecognized database version: {}", user_version));
                }
            }
        }
        // Lock is released when lock_file is dropped
        Ok(Self { conn: Mutex::new(conn) })
    }

    #[tracing::instrument]
    pub async fn save(self) -> anyhow::Result<()> {
        // do some cleanup in the future
        Ok(())
    }

    /// Try to hit cache with spawn fingerprint.
    /// Returns `Ok(Ok(cache_value))` on cache hit, `Ok(Err(cache_miss))` on miss.
    pub async fn try_hit(
        &self,
        cache_metadata: &CacheMetadata,
        base_dir: &AbsolutePath,
    ) -> anyhow::Result<Result<CommandCacheValue, CacheMiss>> {
        let spawn_fingerprint = &cache_metadata.spawn_fingerprint;
        let execution_cache_key = &cache_metadata.execution_cache_key;

        // Try to directly find the cache by spawn fingerprint first
        if let Some(cache_value) = self.get_by_spawn_fingerprint(spawn_fingerprint).await? {
            // Validate post-run fingerprint
            if let Some(post_run_fingerprint_mismatch) =
                cache_value.post_run_fingerprint.validate(base_dir)?
            {
                // Found the cache with the same spawn fingerprint, but the post-run fingerprint mismatches
                return Ok(Err(CacheMiss::FingerprintMismatch(
                    FingerprintMismatch::PostRunFingerprintMismatch(post_run_fingerprint_mismatch),
                )));
            }
            // Associate the execution key to the spawn fingerprint if not already,
            // so that next time we can find it and report spawn fingerprint mismatch
            self.upsert_execution_key_to_fingerprint(execution_cache_key, spawn_fingerprint)
                .await?;
            return Ok(Ok(cache_value));
        }

        // No cache found with the current spawn fingerprint,
        // check if execution key maps to different fingerprint
        if let Some(old_spawn_fingerprint) =
            self.get_fingerprint_by_execution_key(execution_cache_key).await?
        {
            // Found a spawn fingerprint associated with the same execution key,
            // meaning the command or env has changed since last run
            return Ok(Err(CacheMiss::FingerprintMismatch(
                FingerprintMismatch::SpawnFingerprintMismatch {
                    old: old_spawn_fingerprint,
                    new: spawn_fingerprint.clone(),
                },
            )));
        }

        Ok(Err(CacheMiss::NotFound))
    }

    /// Update cache after successful execution.
    pub async fn update(
        &self,
        cache_metadata: &CacheMetadata,
        cache_value: CommandCacheValue,
    ) -> anyhow::Result<()> {
        let spawn_fingerprint = &cache_metadata.spawn_fingerprint;
        let execution_cache_key = &cache_metadata.execution_cache_key;

        self.upsert_spawn_fingerprint_cache(spawn_fingerprint, &cache_value).await?;
        self.upsert_execution_key_to_fingerprint(execution_cache_key, spawn_fingerprint).await?;
        Ok(())
    }
}

// Basic database operations
impl ExecutionCache {
    async fn get_key_by_value<K: Encode, V: Decode<()>>(
        &self,
        table: &str,
        key: &K,
    ) -> anyhow::Result<Option<V>> {
        let conn = self.conn.lock().await;
        let mut select_stmt =
            conn.prepare_cached(&format!("SELECT value FROM {table} WHERE key=?"))?;
        let key_blob = encode_to_vec(key, BINCODE_CONFIG)?;
        let Some(value_blob) =
            select_stmt.query_row::<Vec<u8>, _, _>([key_blob], |row| row.get(0)).optional()?
        else {
            return Ok(None);
        };
        let (value, _) = decode_from_slice::<V, _>(&value_blob, BINCODE_CONFIG)?;
        Ok(Some(value))
    }

    async fn get_by_spawn_fingerprint(
        &self,
        spawn_fingerprint: &SpawnFingerprint,
    ) -> anyhow::Result<Option<CommandCacheValue>> {
        self.get_key_by_value("spawn_fingerprint_cache", spawn_fingerprint).await
    }

    async fn get_fingerprint_by_execution_key(
        &self,
        execution_cache_key: &ExecutionCacheKey,
    ) -> anyhow::Result<Option<SpawnFingerprint>> {
        self.get_key_by_value("execution_key_to_fingerprint", execution_cache_key).await
    }

    async fn upsert<K: Encode, V: Encode>(
        &self,
        table: &str,
        key: &K,
        value: &V,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let key_blob = encode_to_vec(key, BINCODE_CONFIG)?;
        let value_blob = encode_to_vec(value, BINCODE_CONFIG)?;
        let mut update_stmt = conn.prepare_cached(&format!(
            "INSERT INTO {table} (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2"
        ))?;
        update_stmt.execute([key_blob, value_blob])?;
        Ok(())
    }

    async fn upsert_spawn_fingerprint_cache(
        &self,
        spawn_fingerprint: &SpawnFingerprint,
        cache_value: &CommandCacheValue,
    ) -> anyhow::Result<()> {
        self.upsert("spawn_fingerprint_cache", spawn_fingerprint, cache_value).await
    }

    async fn upsert_execution_key_to_fingerprint(
        &self,
        execution_cache_key: &ExecutionCacheKey,
        spawn_fingerprint: &SpawnFingerprint,
    ) -> anyhow::Result<()> {
        self.upsert("execution_key_to_fingerprint", execution_cache_key, spawn_fingerprint).await
    }

    async fn list_table<K: Decode<()> + Serialize, V: Decode<()> + Serialize>(
        &self,
        table: &str,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let mut select_stmt = conn.prepare_cached(&format!("SELECT key, value FROM {table}"))?;
        let mut rows = select_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key_blob: Vec<u8> = row.get(0)?;
            let value_blob: Vec<u8> = row.get(1)?;
            let (key, _) = decode_from_slice::<K, _>(&key_blob, BINCODE_CONFIG)?;
            let (value, _) = decode_from_slice::<V, _>(&value_blob, BINCODE_CONFIG)?;
            writeln!(
                out,
                "{} => {}",
                serde_json::to_string_pretty(&key)?,
                serde_json::to_string_pretty(&value)?
            )?;
        }
        Ok(())
    }

    pub async fn list(&self, mut out: impl Write) -> anyhow::Result<()> {
        out.write_all(b"------- execution_key_to_fingerprint -------\n")?;
        self.list_table::<ExecutionCacheKey, SpawnFingerprint>(
            "execution_key_to_fingerprint",
            &mut out,
        )
        .await?;
        out.write_all(b"------- spawn_fingerprint_cache -------\n")?;
        self.list_table::<SpawnFingerprint, CommandCacheValue>("spawn_fingerprint_cache", &mut out)
            .await?;
        Ok(())
    }
}

//! Execution cache for storing and retrieving cached command results.

pub mod archive;
pub mod display;
pub mod remote;
mod validation;

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fmt::Display,
    fs::File,
    io::Write,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

// Re-export display functions for convenience
pub use display::{format_cache_status_inline, format_change};
use rusqlite::{Connection, OptionalExtension as _};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;
use vt_casefold::EnvName;
use vt_glob::path::PathGlobSet;
use vt_graph::config::ResolvedGlobConfig;
use vt_path::{AbsolutePath, AbsolutePathBuf, RelativePathBuf};
use vt_plan::{
    cache_metadata::{CacheMetadata, ExecutionCacheKey, SpawnFingerprint},
    remote_cache::{RemoteCacheAccess, ResolvedRemoteCacheConfig},
};
use vt_remote_cache::Fetched;
use vt_str::Str;
use wincode::{
    SchemaRead, SchemaReadOwned, SchemaWrite,
    config::Configuration,
    error::{ReadResult, WriteResult},
};

use self::remote::{ReadError, RemoteClients, RemoteUploads, Restore, UploadError};
use super::{
    event::RunKind,
    execute::{
        fingerprint::{PathFingerprint, PostRunFingerprint},
        pipe::StdOutput,
    },
};

const TASK_CACHE_PREALLOCATION_SIZE_LIMIT: usize = 256 * 1024 * 1024;
type TaskCacheConfig = Configuration<true, TASK_CACHE_PREALLOCATION_SIZE_LIMIT>;
const TASK_CACHE_CONFIG: TaskCacheConfig =
    Configuration::default().with_preallocation_size_limit::<TASK_CACHE_PREALLOCATION_SIZE_LIMIT>();

fn serialize_cache<T>(value: &T) -> WriteResult<Vec<u8>>
where
    T: SchemaWrite<TaskCacheConfig, Src = T> + ?Sized,
{
    wincode::config::serialize(value, TASK_CACHE_CONFIG)
}

fn deserialize_cache<T>(bytes: &[u8]) -> ReadResult<T>
where
    T: SchemaReadOwned<TaskCacheConfig, Dst = T>,
{
    wincode::config::deserialize_exact(bytes, TASK_CACHE_CONFIG)
}

/// Cache lookup key identifying a task's execution configuration.
///
/// # Key vs value design
///
/// Put a field in the **key** if each distinct value should have its own
/// cache entry (e.g., different env values → different entries, so
/// reverting an env change can still hit the old entry).
///
/// Put a field in the **value** ([`CacheEntryValue`]) if changes should
/// overwrite the existing entry (e.g., input file hashes — there's no
/// reason to keep the old hash around, and storing them in the value
/// lets us report exactly *which file* changed).
#[derive(Debug, SchemaWrite, SchemaRead, Serialize, PartialEq, Eq, Clone)]
pub struct CacheEntryKey {
    /// The spawn fingerprint (command, args, cwd, envs)
    pub spawn_fingerprint: SpawnFingerprint,
    /// Resolved input configuration that affects cache behavior.
    /// Glob patterns are workspace-root-relative.
    pub input_config: ResolvedGlobConfig,
    /// Resolved output configuration that affects cache restoration.
    /// Glob patterns are workspace-root-relative.
    pub output_config: ResolvedGlobConfig,
}

impl CacheEntryKey {
    fn from_metadata(cache_metadata: &CacheMetadata) -> Self {
        Self {
            spawn_fingerprint: cache_metadata.spawn_fingerprint.clone(),
            input_config: cache_metadata.input_config.clone(),
            output_config: cache_metadata.output_config.clone(),
        }
    }
}

/// Cached execution result for a task.
///
/// Contains the post-run fingerprint (from fspy), captured outputs,
/// execution duration, and explicit input file hashes.
#[derive(Debug, SchemaWrite, SchemaRead, Serialize)]
pub struct CacheEntryValue {
    pub post_run_fingerprint: PostRunFingerprint,
    pub std_outputs: Arc<[StdOutput]>,
    pub duration: Duration,
    /// Hashes of explicit input files computed from positive globs.
    /// Files matching negative globs are already filtered out.
    /// Path is relative to workspace root, value is `xxHash3_64` of file content.
    /// Stored in the value (not the key) so changes can be detected and reported.
    pub globbed_inputs: BTreeMap<RelativePathBuf, u64>,
    /// Filename of the output archive (e.g. `{uuid}.tar.zst`) stored alongside
    /// `cache.db` in the cache directory. `None` if no output files were produced.
    pub output_archive: Option<Str>,
}

#[derive(Debug)]
pub struct ExecutionCache {
    conn: Mutex<Connection>,
    /// The folder that holds the database and the output archives.
    dir: AbsolutePathBuf,
    remote_clients: RemoteClients,
    uploads: RemoteUploads,
}

/// What `vp run -vv` shows of a cached run: everything it depends on, changed
/// or not, and its outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineDetails {
    /// The env names it depends on, sorted: its `cache.env`, and those that
    /// runner-aware tools read or matched with a tracked query.
    pub envs: Vec<Str>,
    /// Its inputs, by path, with what is checked: the files it matched or
    /// read, the folders it opened, and the paths it found missing.
    pub inputs: BTreeMap<RelativePathBuf, BaselinePath>,
    /// The output files a hit restores. `None` when its archive couldn't be
    /// read.
    pub outputs: Option<BaselineOutputs>,
}

/// The output files of a cached run, which a hit restores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaselineOutputs {
    /// The files, sorted. Empty when the run has no outputs.
    Listed(Vec<Str>),
    /// The run has outputs, but in an archive in the remote cache that wasn't
    /// downloaded, as in a dry run, so they can't be listed.
    NotDownloaded,
}

/// The files in the output archive at `path`, or `None` if it can't be read.
fn list_outputs(path: &AbsolutePath) -> Option<BaselineOutputs> {
    archive::list_output_files(path)
        .inspect_err(|err| tracing::debug!(?err, "failed to list the outputs"))
        .ok()
        .map(BaselineOutputs::Listed)
}

/// What a cached run recorded of a path it depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaselinePath {
    /// A file, with a hash of its content. The task rewrites a file that may
    /// be missing.
    File { hash: u64, may_be_missing: bool },
    /// A folder, with its entries, sorted, if the task listed it. The task
    /// creates a folder that may be missing.
    Dir { entries: Option<Vec<Str>>, may_be_missing: bool },
    /// A path the task looked for and didn't find.
    Missing,
}

/// A cache hit: the entry to replay, and the cache it came from.
#[derive(Debug)]
pub struct CacheHit {
    pub value: CacheEntryValue,
    pub source: CacheHitSource,
    /// With `vp run -vv`, what the entry depends on and its outputs.
    pub baseline: Option<Arc<BaselineDetails>>,
}

/// The cache a hit came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheHitSource {
    /// The local cache.
    #[default]
    Local,
    /// The remote cache. The entry is recorded locally once its outputs are
    /// restored.
    Remote,
}

#[derive(Debug, Clone)]
pub enum CacheMiss {
    NotFound,
    /// The cache has a run of the task to compare with, and it differs.
    Changed(Comparison),
    /// Reading the remote cache failed, and the local cache has no entry for
    /// the task.
    RemoteReadFailed(Arc<ReadError>),
}

/// How the current execution differs from the cached run a lookup compared
/// it with.
#[derive(Debug, Clone)]
pub struct Comparison {
    /// Every difference found, sorted by kind, then by name or path, never
    /// empty. Command, config and env changes are complete. Without `-v`, the
    /// file check stops at the first changed file it finds, so at most one
    /// file is listed.
    pub changes: Vec<Change>,
    /// How long the cached run took. `None` when the command, config or env
    /// changed: then only the key of the task's previous cached run is
    /// compared, not the run itself.
    pub baseline_duration: Option<Duration>,
    /// Other files may have changed too: the file check stopped at the first
    /// changed file or couldn't check some of them, or, after a command,
    /// config or env change, didn't run.
    pub changes_partial: bool,
    /// With `vp run -vv`, what the cached run depends on and its outputs.
    /// `None` after a command, config or env change, when only the previous
    /// run's key is compared.
    pub baseline: Option<Arc<BaselineDetails>>,
}

impl Comparison {
    /// Sort `changes` and drop duplicates, such as an env var that two
    /// tracked queries match. Of two duplicates, the one found first is kept.
    fn new(
        mut changes: Vec<Change>,
        baseline_duration: Option<Duration>,
        changes_partial: bool,
    ) -> Self {
        changes.sort_by(|a, b| a.order_key().cmp(&b.order_key()));
        changes.dedup_by(|a, b| a.order_key() == b.order_key());
        Self { changes, baseline_duration, changes_partial, baseline: None }
    }

    /// The miss for a cached run, `value`, that differs from the execution
    /// as `checked` found.
    fn of_value(value: &CacheEntryValue, checked: Checked) -> Self {
        Self::new(checked.changes, Some(value.duration), checked.partial)
    }

    /// The miss for a task whose previous cached run is stored under
    /// `old_key`: only the keys are compared, so the files and tracked envs
    /// aren't checked.
    fn of_key(old_key: &CacheEntryKey, current_key: &CacheEntryKey) -> Self {
        Self::new(old_key.changes_to(current_key), None, true)
    }

    /// The comparison, with `baseline` describing the cached run.
    fn with_baseline(self, baseline: Option<Arc<BaselineDetails>>) -> Self {
        Self { baseline, ..self }
    }
}

/// What comparing a cached run with the execution found.
#[derive(Debug)]
pub struct Checked {
    /// Every difference found. Empty when the cached run matches.
    pub changes: Vec<Change>,
    /// Some files weren't checked, as in [`Comparison::changes_partial`].
    pub partial: bool,
}

/// One difference between the current execution and the cached run it's
/// compared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// The program the command runs changed. Each is the program as the
    /// cache records it: its path inside the workspace, or just its name
    /// outside it.
    Program { from: Str, to: Str },
    /// The command's arguments changed.
    Args { from: Arc<[Str]>, to: Arc<[Str]> },
    /// The directory the command runs in changed. Each is relative to the
    /// workspace root.
    Cwd { from: RelativePathBuf, to: RelativePathBuf },
    /// The task's `input` setting changed.
    InputConfig,
    /// The task's `output` setting changed.
    OutputConfig,
    /// A name was added to the task's `untrackedEnv` setting.
    UntrackedEnvAdded { name: Str },
    /// A name was removed from the task's `untrackedEnv` setting.
    UntrackedEnvRemoved { name: Str },
    /// An env var was added, removed, or changed value.
    Env { mismatch: EnvMismatch, source: EnvSource },
    /// An input file or folder was added, removed, or modified.
    File { kind: InputChangeKind, path: RelativePathBuf },
}

impl Change {
    /// The order changes are listed in: by kind in the order of the
    /// variants, then by name or path, then by how it changed. Env names
    /// compare by the platform's rules, so on Windows a tool's `getEnv('a')`
    /// and a query matching `A` name the same env.
    fn order_key(&self) -> (u8, OrderName<'_>, u8) {
        let exact = OrderName::Exact;
        match self {
            Self::Program { .. } => (0, exact(""), 0),
            Self::Args { .. } => (1, exact(""), 0),
            Self::Cwd { .. } => (2, exact(""), 0),
            Self::InputConfig => (3, exact(""), 0),
            Self::OutputConfig => (4, exact(""), 0),
            Self::UntrackedEnvAdded { name } => (5, exact(name), 0),
            Self::UntrackedEnvRemoved { name } => (5, exact(name), 1),
            Self::Env { mismatch, .. } => {
                let how = match mismatch {
                    EnvMismatch::Added { .. } => 0,
                    EnvMismatch::Removed { .. } => 1,
                    EnvMismatch::Changed { .. } => 2,
                };
                (6, OrderName::Env(EnvName::new(mismatch.name().as_str())), how)
            }
            Self::File { kind, path } => {
                let how = match kind {
                    InputChangeKind::ContentModified => 0,
                    InputChangeKind::Added => 1,
                    InputChangeKind::Removed => 2,
                };
                (7, exact(path.as_str()), how)
            }
        }
    }
}

/// A change's name or path, as [`Change::order_key`] compares it.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum OrderName<'a> {
    Exact(&'a str),
    Env(EnvName<&'a str>),
}

/// How far a cache lookup checks a cached run's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCheck {
    /// Stop at the first changed file, which is enough to know it's a miss.
    First,
    /// Check every file, so the miss lists every change (`vp run -v`).
    All,
    /// Check every file, and describe the cached run: everything it depends
    /// on, changed or not, and its outputs (`vp run -vv`).
    AllAndBaseline,
}

impl FileCheck {
    /// The check that `-v` given `verbose` times asks for.
    #[must_use]
    pub const fn from_verbosity(verbose: u8) -> Self {
        match verbose {
            0 => Self::First,
            1 => Self::All,
            _ => Self::AllAndBaseline,
        }
    }
}

/// Where the cache records an env var.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvSource {
    /// The task's `cache.env` names it, so its value is part of the cache
    /// key.
    Fingerprinted,
    /// A runner-aware tool read it with `tracked: true`, by name or with a
    /// query.
    Tracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputChangeKind {
    /// File content changed but path is the same
    ContentModified,
    /// New file or folder added
    Added,
    /// Existing file or folder removed
    Removed,
}

/// A single env var difference between a stored fingerprint and the current
/// environment.
///
/// The canonical shape for an env change wherever one is detected and
/// reported. The [`Display`] impl is the single source of the user-facing
/// wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvMismatch {
    /// Set now, but absent from the stored fingerprint.
    Added { name: Str },
    /// In the stored fingerprint, but unset now.
    Removed { name: Str },
    /// Present on both sides with different values.
    Changed { name: Str },
}

impl EnvMismatch {
    /// The name of the env var that diverged.
    #[must_use]
    pub const fn name(&self) -> &Str {
        match self {
            Self::Added { name } | Self::Removed { name } | Self::Changed { name } => name,
        }
    }

    /// Compare a stored env value against the current one, returning the
    /// mismatch if they differ. `None` on either side means the env is unset
    /// there; two unset or two equal values are not a mismatch.
    #[must_use]
    pub fn compare<T: Eq>(name: &Str, stored: Option<&T>, current: Option<&T>) -> Option<Self> {
        match (stored, current) {
            (None, Some(_)) => Some(Self::Added { name: name.clone() }),
            (Some(_), None) => Some(Self::Removed { name: name.clone() }),
            (Some(old_value), Some(new_value)) if old_value != new_value => {
                Some(Self::Changed { name: name.clone() })
            }
            _ => None,
        }
    }

    /// Every env added, removed or changed between `stored` and `current`, in
    /// name order: those in `stored` first, then those only in `current`.
    pub fn diff<'a, T: Eq>(
        stored: &'a BTreeMap<Str, T>,
        current: &'a BTreeMap<Str, T>,
    ) -> impl Iterator<Item = Self> + 'a {
        let added = current.keys().filter(|name| !stored.contains_key(*name));
        stored
            .keys()
            .chain(added)
            .filter_map(|name| Self::compare(name, stored.get(name), current.get(name)))
    }
}

impl Display for EnvMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Added { name } => write!(f, "env '{name}' added"),
            Self::Removed { name } => write!(f, "env '{name}' removed"),
            Self::Changed { name } => write!(f, "env '{name}' changed"),
        }
    }
}

/// Split a relative path into `(parent_dir, filename)`.
/// Returns `None` for the parent if the path has no `/` separator.
pub fn split_path(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((parent, filename)) => (Some(parent), filename),
        None => (None, path),
    }
}

/// On-disk schema version of the cache database.
///
/// Bump this whenever the database layout (table structure, serialization
/// format, or fingerprint semantics) changes in an incompatible way.
///
/// The version is encoded *only* in the cache directory name (see
/// [`cache_schema_dir_name`], e.g. `v14`); there is no in-database version
/// marker. Keying the storage location on this version means Vite+ builds that
/// pin different schema versions never open each other's database: each keeps
/// its own cache warm across branch switches, and a cache from a different
/// version is simply ignored (it lives in a directory this build never looks
/// at) rather than aborting the run. Bumping the version starts a fresh cache.
const CACHE_SCHEMA_VERSION: u32 = 23;

/// Name of the per-version subdirectory (e.g. `v14`) under the task-cache
/// directory that holds the database and output archives for the current
/// [`CACHE_SCHEMA_VERSION`].
pub fn cache_schema_dir_name() -> Str {
    vt_str::format!("v{CACHE_SCHEMA_VERSION}")
}

/// Switches `conn` to WAL mode and, if that worked, sets `synchronous=NORMAL`.
///
/// With `NORMAL`, commits don't wait for a sync; SQLite syncs the WAL at checkpoints instead.
/// In WAL mode, a power loss can roll back the latest commits (their entries become cache
/// misses) but can't corrupt the database.
///
/// `PRAGMA journal_mode=WAL` doesn't return an error when SQLite can't use WAL. It keeps the
/// current mode, usually a rollback journal, and returns that mode's name. With a rollback
/// journal, `NORMAL` has a small chance of corrupting the database after a power loss, so
/// `synchronous` stays `FULL` unless the result is `wal`.
fn enable_wal(conn: &Connection) -> rusqlite::Result<()> {
    let is_wal = conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
        Ok(row.get_ref(0)?.as_str()?.eq_ignore_ascii_case("wal"))
    })?;
    if is_wal {
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

/// Run `work` on rayon's global thread pool, in the current span, and wait for
/// it until `cancel_token` is cancelled.
///
/// The scheduler polls every task's future on one thread, so file and CPU work
/// done there, such as hashing a task's inputs or checking a cache entry,
/// holds up every other task until it's done. Run there, the cache lookups of
/// tasks that become ready together would run one after another.
///
/// The pool is rayon's rather than tokio's blocking threads because checking
/// an entry already hashes its inferred inputs in parallel on it, and a pool
/// thread takes part in that work instead of waiting for it. With one thread
/// (`RAYON_NUM_THREADS=1`, as in the e2e tests), work also finishes in the
/// order it's submitted, so tasks that become ready together start in the
/// same order every run.
///
/// Returns `None` if `cancel_token` is cancelled first. Work that hasn't
/// started then doesn't run, and work that's already running finishes in the
/// background with its result dropped, so it must have no effects that
/// outlive it. If `work` panics, the panic is resumed here.
pub async fn run_off_thread<T: Send + 'static>(
    cancel_token: &CancellationToken,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if cancel_token.is_cancelled() {
        return None;
    }
    let span = tracing::Span::current();
    let (sender, receiver) = oneshot::channel();
    rayon::spawn_fifo(move || {
        if sender.is_closed() {
            return;
        }
        // A panic in a job would abort the process, so it's sent to the
        // waiting task instead.
        let _ = sender.send(panic::catch_unwind(AssertUnwindSafe(|| span.in_scope(work))));
    });
    match cancel_token.run_until_cancelled(receiver).await? {
        Ok(Ok(output)) => Some(output),
        Ok(Err(panic)) => panic::resume_unwind(panic),
        // The job sends a result unless the receiver is dropped first.
        Err(oneshot::error::RecvError { .. }) => None,
    }
}

/// What a lookup validates entries against: the execution's current envs and
/// explicit inputs, and the files under the workspace root. It owns all of
/// them, so that entries can be checked off the scheduler's thread.
#[derive(Clone)]
struct Validation {
    /// The execution's `CacheMetadata::unfiltered_envs`.
    unfiltered_envs: Arc<FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>>,
    globbed_inputs: Arc<BTreeMap<RelativePathBuf, u64>>,
    input_negative_globs: Arc<PathGlobSet<'static>>,
    workspace_root: Arc<AbsolutePath>,
    /// How many changed files to look for.
    file_check: FileCheck,
}

impl Validation {
    /// Validate `cache_value` as [`CacheEntryValue::validate`] does.
    fn validate(&self, cache_value: &CacheEntryValue) -> anyhow::Result<Checked> {
        cache_value.validate(
            &self.unfiltered_envs,
            &self.globbed_inputs,
            &self.input_negative_globs,
            &self.workspace_root,
            self.file_check,
        )
    }
}

impl ExecutionCache {
    #[tracing::instrument(level = "debug", skip_all)]
    pub fn load_from_path(path: &AbsolutePath) -> anyhow::Result<Self> {
        tracing::info!("Creating task cache directory at {}", path.as_path().display());
        std::fs::create_dir_all(path)?;

        // Use file lock to prevent race conditions when multiple processes initialize the database
        let lock_path = path.join("db_open.lock");
        let lock_file = File::create(lock_path.as_path())?;
        lock_file.lock()?;

        let db_path = path.join("cache.db");
        let conn = Connection::open(db_path.as_path())?;
        enable_wal(&conn)?;
        // The schema version is encoded in the directory name (see
        // `cache_schema_dir_name`), so any database in this directory already has
        // the current schema: there is nothing to migrate or version-check. Ensure
        // the tables exist in a single round-trip. On an existing database the
        // `IF NOT EXISTS` creates are near-free no-ops (a schema lookup, no write);
        // on a fresh one they create the tables. This runs once per process (the
        // cache is `OnceCell`-initialized).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cache_entries (key BLOB PRIMARY KEY, value BLOB);
             CREATE TABLE IF NOT EXISTS task_fingerprints (key BLOB PRIMARY KEY, value BLOB);",
        )?;
        // Lock is released when lock_file is dropped
        Ok(Self {
            conn: Mutex::new(conn),
            dir: path.to_absolute_path_buf(),
            remote_clients: RemoteClients::default(),
            uploads: RemoteUploads::default(),
        })
    }

    #[tracing::instrument]
    pub async fn save(self) -> anyhow::Result<()> {
        // do some cleanup in the future
        Ok(())
    }

    /// Try to hit cache by looking up the cache entry key and validating inputs.
    /// Returns `Ok(Some(Ok(cache_hit)))` on cache hit, `Ok(Some(Err(cache_miss)))`
    /// on miss.
    ///
    /// After a local miss, the remote cache is queried if the task has one. A
    /// remote hit has its output archive downloaded into `cache_dir`, is
    /// recorded locally by [`Self::restore`], and is never uploaded. If the
    /// local cache has an entry for the task, its miss reason is kept.
    /// Otherwise the reason comes from the remote cache.
    ///
    /// Entries are decoded and validated on rayon's pool, as
    /// [`run_off_thread`] describes, so other tasks' lookups don't wait for
    /// them. The lookup stops when `cancel_token` is cancelled. A cancelled
    /// remote request or check is a [`ReadError::Cancelled`] miss, and a
    /// cancelled check of the local entry returns `Ok(None)`. Either way the
    /// task doesn't start, so nothing is reported.
    ///
    /// A dry run (`RunKind::Dry`) writes nothing: a local hit isn't recorded,
    /// and a remote hit's archive isn't downloaded. `file_check` says how many
    /// changed files a miss lists.
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(clippy::too_many_arguments, reason = "each argument is part of one lookup")]
    pub async fn try_hit(
        &self,
        cache_metadata: &CacheMetadata,
        globbed_inputs: &Arc<BTreeMap<RelativePathBuf, u64>>,
        workspace_root: &Arc<AbsolutePath>,
        cache_dir: &AbsolutePath,
        run_kind: RunKind,
        file_check: FileCheck,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<Option<Result<CacheHit, CacheMiss>>> {
        let cache_key = CacheEntryKey::from_metadata(cache_metadata);
        let validation = Validation {
            unfiltered_envs: Arc::clone(&cache_metadata.unfiltered_envs),
            globbed_inputs: Arc::clone(globbed_inputs),
            // Part of the key, so the same as when the entry was created.
            input_negative_globs: Arc::new(
                PathGlobSet::new(&cache_metadata.input_config.negative_globs)?.into_owned(),
            ),
            workspace_root: Arc::clone(workspace_root),
            file_check,
        };

        let start = Instant::now();
        let Some(local_lookup) = self
            .try_hit_local(cache_metadata, &cache_key, &validation, run_kind, cancel_token)
            .await?
        else {
            return Ok(None);
        };
        tracing::debug!(
            outcome = match &local_lookup {
                Ok(_) => "hit",
                Err(CacheMiss::NotFound) => "not_found",
                Err(_) => "mismatch",
            },
            elapsed = ?start.elapsed(),
            "looked up the local cache"
        );
        let local_miss = match local_lookup {
            Ok(hit) => return Ok(Some(Ok(hit))),
            Err(miss) => miss,
        };
        #[expect(
            clippy::manual_let_else,
            reason = "naming every access mode makes adding one a compile error here"
        )]
        let remote_config = match &cache_metadata.remote_cache {
            Some(
                remote_config @ ResolvedRemoteCacheConfig {
                    access: RemoteCacheAccess::Read | RemoteCacheAccess::ReadWrite,
                    ..
                },
            ) => remote_config,
            None => return Ok(Some(Err(local_miss))),
        };
        let start = Instant::now();
        let remote_lookup = self
            .try_hit_remote(
                remote_config,
                cache_metadata,
                &cache_key,
                &validation,
                cache_dir,
                run_kind,
                cancel_token,
            )
            .await?;
        tracing::debug!(
            outcome = match &remote_lookup {
                Ok(_) => "hit",
                Err(CacheMiss::NotFound) => "not_found",
                Err(CacheMiss::Changed(_)) => "mismatch",
                Err(CacheMiss::RemoteReadFailed(_)) => "read_failed",
            },
            elapsed = ?start.elapsed(),
            "looked up the remote cache"
        );
        let remote_miss = match remote_lookup {
            Ok(hit) => return Ok(Some(Ok(hit))),
            Err(miss) => miss,
        };
        Ok(Some(Err(match local_miss {
            CacheMiss::NotFound => remote_miss,
            local_miss => local_miss,
        })))
    }

    /// Look up the local entry under `cache_key` and validate it with
    /// `validation`. Returns `None` if `cancel_token` is cancelled while the
    /// entry is checked.
    async fn try_hit_local(
        &self,
        cache_metadata: &CacheMetadata,
        cache_key: &CacheEntryKey,
        validation: &Validation,
        run_kind: RunKind,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<Option<Result<CacheHit, CacheMiss>>> {
        let execution_cache_key = &cache_metadata.execution_cache_key;

        // Try to find the cache entry by key (spawn fingerprint + input config)
        if let Some(value_blob) = self.get_cache_entry_blob(cache_key).await? {
            let (validation, key, dir) = (validation.clone(), cache_key.clone(), self.dir.clone());
            let Some(checked) = run_off_thread(cancel_token, move || {
                let cache_value: CacheEntryValue = deserialize_cache(&value_blob)?;
                let checked = validation.validate(&cache_value)?;
                let baseline = Self::describe(
                    validation.file_check,
                    &key,
                    &cache_value,
                    CacheHitSource::Local,
                    &dir,
                );
                anyhow::Ok((cache_value, checked, baseline))
            })
            .await
            else {
                return Ok(None);
            };
            let (cache_value, checked, baseline) = checked?;
            if !checked.changes.is_empty() {
                let comparison = Comparison::of_value(&cache_value, checked);
                return Ok(Some(Err(CacheMiss::Changed(comparison.with_baseline(baseline)))));
            }
            // Associate the execution key to the cache entry key if not already,
            // so that next time we can find it and report what changed
            if run_kind == RunKind::Real {
                self.upsert_task_fingerprint(execution_cache_key, cache_key).await?;
            }
            return Ok(Some(Ok(CacheHit {
                value: cache_value,
                source: CacheHitSource::Local,
                baseline,
            })));
        }

        // No cache found with the current cache entry key,
        // check if execution key maps to a different cache entry key
        if let Some(old_cache_key) =
            self.get_cache_key_by_execution_key(execution_cache_key).await?
        {
            // `get_cache_entry_blob` above returned None for the *current*
            // cache key, so the associated key must differ.
            return Ok(Some(Err(CacheMiss::Changed(Comparison::of_key(
                &old_cache_key,
                cache_key,
            )))));
        }

        Ok(Some(Err(CacheMiss::NotFound)))
    }

    /// Fetch the entry from the remote cache that `remote_config` configures.
    /// An exact entry that passes validation is a hit once its output archive
    /// is downloaded, or right away in a dry run.
    /// A fallback entry, a failed validation, or a failed read is a miss. An
    /// error while validating counts as a failed read, so the remote entry
    /// never fails the task. The fetched entries are checked off the
    /// scheduler's thread.
    #[expect(clippy::too_many_arguments, reason = "forwarded from `try_hit`")]
    async fn try_hit_remote(
        &self,
        remote_config: &ResolvedRemoteCacheConfig,
        cache_metadata: &CacheMetadata,
        cache_key: &CacheEntryKey,
        validation: &Validation,
        cache_dir: &AbsolutePath,
        run_kind: RunKind,
        cancel_token: &CancellationToken,
    ) -> anyhow::Result<Result<CacheHit, CacheMiss>> {
        let start = Instant::now();
        let fetched = self
            .remote_clients
            .fetch(remote_config, cache_key, &cache_metadata.execution_cache_key, cancel_token)
            .await;
        let (outcome, entries) = match &fetched {
            Ok(Some(Fetched::Exact { entries })) => ("exact", entries.len()),
            Ok(Some(Fetched::Fallback { .. })) => ("fallback", 0),
            Ok(None) => ("not_found", 0),
            Err(_) => ("failed", 0),
        };
        tracing::debug!(
            outcome,
            entries,
            error = fetched.as_ref().err().map(tracing::field::debug),
            elapsed = ?start.elapsed(),
            "fetched from the remote cache"
        );
        let (cache_key, validation, dir) =
            (cache_key.clone(), validation.clone(), self.dir.clone());
        let check = move || {
            remote::resolve(
                fetched,
                &cache_key,
                |cache_value| validation.validate(cache_value),
                |cache_value| {
                    Self::describe(
                        validation.file_check,
                        &cache_key,
                        cache_value,
                        CacheHitSource::Remote,
                        &dir,
                    )
                },
            )
        };
        let resolved = run_off_thread(cancel_token, check).await;
        let Restore { value: cache_value, blob_id, baseline } = match resolved {
            Some(Ok(restore)) => restore,
            Some(Err(miss)) => return Ok(Err(miss)),
            None => return Ok(Err(ReadError::Cancelled.into_miss())),
        };
        // `resolve` checks that the entry has an archive exactly when it has a
        // blob to download, so the hit a dry run shows says whether it restores
        // outputs.
        if run_kind == RunKind::Dry {
            return Ok(Ok(CacheHit {
                value: cache_value,
                source: CacheHitSource::Remote,
                baseline,
            }));
        }

        let output_archive = match blob_id {
            Some(blob_id) => {
                match self
                    .remote_clients
                    .download_archive(remote_config, &blob_id, cache_dir, cancel_token)
                    .await
                {
                    Ok(archive_name) => Some(archive_name),
                    Err(err) => return Ok(Err(err.into_miss())),
                }
            }
            None => None,
        };
        // Now that the archive is here, `-vv` lists its files, as for a local
        // hit. If the run is cancelled meanwhile, it isn't reported.
        let baseline = match (baseline, &output_archive) {
            (Some(details), Some(archive)) => {
                let archive_path = cache_dir.join(archive.as_str());
                let outputs = run_off_thread(cancel_token, move || list_outputs(&archive_path))
                    .await
                    .flatten();
                Some(Arc::new(BaselineDetails { outputs, ..BaselineDetails::clone(&details) }))
            }
            (baseline, _) => baseline,
        };
        let value = CacheEntryValue { output_archive, ..cache_value };
        Ok(Ok(CacheHit { value, source: CacheHitSource::Remote, baseline }))
    }

    /// With [`FileCheck::AllAndBaseline`], what `vp run -vv` shows of the
    /// cached run `value`, stored under `key` in the cache `source`. A local
    /// run's outputs are listed from its archive in `dir`, the local cache's
    /// folder. A remote run's archive isn't downloaded yet, so its outputs are
    /// [`BaselineOutputs::NotDownloaded`] until a real run downloads it. It
    /// reads the archive, so lookups call it off the scheduler's thread.
    fn describe(
        file_check: FileCheck,
        key: &CacheEntryKey,
        value: &CacheEntryValue,
        source: CacheHitSource,
        dir: &AbsolutePath,
    ) -> Option<Arc<BaselineDetails>> {
        if file_check != FileCheck::AllAndBaseline {
            return None;
        }
        let fingerprint = &value.post_run_fingerprint;
        // Env names compare by the platform's rules, so on Windows `FOO` in
        // `cache.env` and a tool's `getEnv('foo')` are one name.
        let mut envs: Vec<Str> = key
            .spawn_fingerprint
            .env_fingerprints()
            .fingerprinted_envs
            .keys()
            .chain(fingerprint.tracked_envs.keys())
            .chain(fingerprint.tracked_env_queries.values().flat_map(BTreeMap::keys))
            .cloned()
            .collect();
        envs.sort_by(|a, b| EnvName::new(a.as_str()).cmp(&EnvName::new(b.as_str())));
        envs.dedup_by(|a, b| EnvName::new(a.as_str()) == EnvName::new(b.as_str()));
        let mut inputs: BTreeMap<RelativePathBuf, BaselinePath> = value
            .globbed_inputs
            .iter()
            .map(|(path, hash)| {
                (path.clone(), BaselinePath::File { hash: *hash, may_be_missing: false })
            })
            .collect();
        for (path, input) in &fingerprint.inferred_inputs {
            let recorded = match &input.fingerprint {
                PathFingerprint::FileContentHash(hash) => {
                    BaselinePath::File { hash: *hash, may_be_missing: input.may_be_missing }
                }
                PathFingerprint::Folder(listing) => BaselinePath::Dir {
                    entries: listing
                        .as_ref()
                        .map(|listing| listing.entries.keys().cloned().collect()),
                    may_be_missing: input.may_be_missing,
                },
                PathFingerprint::NotFound => BaselinePath::Missing,
            };
            inputs.insert(path.clone(), recorded);
        }
        let outputs = match (source, &value.output_archive) {
            (_, None) => Some(BaselineOutputs::Listed(Vec::new())),
            (CacheHitSource::Remote, Some(_)) => Some(BaselineOutputs::NotDownloaded),
            (CacheHitSource::Local, Some(archive)) => list_outputs(&dir.join(archive.as_str())),
        };
        Some(Arc::new(BaselineDetails { envs, inputs, outputs }))
    }

    /// Record an entry locally.
    ///
    /// If a previous entry exists for the same cache key with a different
    /// `output_archive`, the stale archive file in `cache_dir` is removed
    /// (best-effort) so it doesn't accumulate on disk.
    async fn record(
        &self,
        cache_key: &CacheEntryKey,
        execution_cache_key: &ExecutionCacheKey,
        cache_value: &CacheEntryValue,
        cache_dir: &AbsolutePath,
    ) -> anyhow::Result<()> {
        // If a previous entry exists with a stale output archive, delete the
        // old file so the cache directory doesn't accumulate orphaned archives.
        if let Some(old_value) = self.get_by_cache_key(cache_key).await?
            && let Some(old_archive) = old_value.output_archive
            && cache_value.output_archive.as_ref() != Some(&old_archive)
        {
            let old_archive_path = cache_dir.join(old_archive.as_str());
            // Best-effort cleanup: a missing file (e.g. after a crash or manual
            // cache clear) is fine, so we ignore the error.
            let _ = std::fs::remove_file(old_archive_path.as_path());
        }

        self.upsert_cache_entry(cache_key, cache_value).await?;
        self.upsert_task_fingerprint(execution_cache_key, cache_key).await?;
        Ok(())
    }

    /// Update cache after successful execution, recording the entry locally
    /// as [`Self::record`] does.
    ///
    /// In `read-write` remote mode, the entry is then uploaded to the remote
    /// cache in the background, and the task doesn't wait for it. Returns
    /// where the upload's error is set if it fails: right away if the upload
    /// can't start, or later by the background upload. Read it after
    /// [`Self::wait_for_uploads`] returns.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn update(
        &self,
        cache_metadata: &CacheMetadata,
        cache_value: CacheEntryValue,
        cache_dir: &AbsolutePath,
    ) -> anyhow::Result<Arc<OnceLock<UploadError>>> {
        let execution_cache_key = &cache_metadata.execution_cache_key;

        let cache_key = CacheEntryKey::from_metadata(cache_metadata);

        let start = Instant::now();
        self.record(&cache_key, execution_cache_key, &cache_value, cache_dir).await?;
        tracing::debug!(elapsed = ?start.elapsed(), "recorded the cache entry locally");

        let upload_error = Arc::new(OnceLock::new());
        let remote_config = match &cache_metadata.remote_cache {
            Some(
                remote_config @ ResolvedRemoteCacheConfig {
                    access: RemoteCacheAccess::ReadWrite,
                    ..
                },
            ) => remote_config,
            Some(ResolvedRemoteCacheConfig { access: RemoteCacheAccess::Read, .. }) | None => {
                return Ok(upload_error);
            }
        };
        match self.remote_clients.prepare_upload(
            remote_config,
            &cache_key,
            execution_cache_key,
            &cache_value,
            cache_dir,
        ) {
            Ok(upload) => self.uploads.spawn(upload, Arc::clone(&upload_error)),
            Err(err) => {
                tracing::debug!(?err, "remote cache upload failed");
                let _ = upload_error.set(err);
            }
        }
        Ok(upload_error)
    }

    /// The number of uploads to the remote cache still running.
    pub fn pending_uploads(&self) -> usize {
        self.uploads.pending()
    }

    /// Wait for the uploads to the remote cache to finish. If
    /// `interrupt_token` is cancelled first, or already was, the uploads are
    /// cancelled instead, each with [`UploadError::Interrupted`] as its error.
    /// The entries stay in the local cache.
    pub async fn wait_for_uploads(&self, interrupt_token: &CancellationToken) {
        self.uploads.wait(interrupt_token).await;
    }

    /// Restore the output files of `hit` into `workspace_root`.
    ///
    /// A remote hit is recorded locally once its outputs are restored. If
    /// they can't be, its downloaded archive is removed instead, so the next
    /// run fetches it again. Returns an error if the outputs can't be
    /// restored.
    pub async fn restore(
        &self,
        cache_metadata: &CacheMetadata,
        hit: &CacheHit,
        workspace_root: &AbsolutePath,
        cache_dir: &AbsolutePath,
    ) -> anyhow::Result<()> {
        let start = Instant::now();
        if let Some(archive_name) = &hit.value.output_archive {
            let archive_path = cache_dir.join(archive_name.as_str());
            if let Err(err) = archive::extract_output_archive(workspace_root, &archive_path) {
                if hit.source == CacheHitSource::Remote {
                    // Best-effort: the file may already be missing.
                    let _ = std::fs::remove_file(archive_path.as_path());
                }
                return Err(err.context("failed to extract the output archive"));
            }
            tracing::debug!(
                bytes = archive::file_size(&archive_path),
                elapsed = ?start.elapsed(),
                "extracted the output archive"
            );
        }

        if hit.source == CacheHitSource::Remote {
            let cache_key = CacheEntryKey::from_metadata(cache_metadata);
            if let Err(err) = self
                .record(&cache_key, &cache_metadata.execution_cache_key, &hit.value, cache_dir)
                .await
            {
                // The outputs are restored, so the task still succeeds. The
                // next run fetches the entry from the remote cache again.
                tracing::warn!(?err, "failed to record a remote cache hit locally");
            }
        }
        tracing::debug!(source = ?hit.source, elapsed = ?start.elapsed(), "restored the outputs");
        Ok(())
    }
}

// Basic database operations
impl ExecutionCache {
    async fn get_key_by_value<
        K: SchemaWrite<TaskCacheConfig, Src = K>,
        V: SchemaReadOwned<TaskCacheConfig, Dst = V>,
    >(
        &self,
        table: &str,
        key: &K,
    ) -> anyhow::Result<Option<V>> {
        let Some(value_blob) = self.get_value_blob(table, key).await? else {
            return Ok(None);
        };
        let value: V = deserialize_cache(&value_blob)?;
        Ok(Some(value))
    }

    /// The encoded value stored under `key` in `table`.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "lock guard cannot be dropped earlier because prepared statement borrows connection"
    )]
    async fn get_value_blob<K: SchemaWrite<TaskCacheConfig, Src = K>>(
        &self,
        table: &str,
        key: &K,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let key_blob = serialize_cache(key)?;
        let conn = self.conn.lock().await;
        #[expect(
            clippy::disallowed_macros,
            reason = "SQL query string for rusqlite requires String"
        )]
        let mut select_stmt =
            conn.prepare_cached(&format!("SELECT value FROM {table} WHERE key=?"))?;
        Ok(select_stmt.query_row::<Vec<u8>, _, _>([key_blob], |row| row.get(0)).optional()?)
    }

    async fn get_by_cache_key(
        &self,
        cache_key: &CacheEntryKey,
    ) -> anyhow::Result<Option<CacheEntryValue>> {
        self.get_key_by_value("cache_entries", cache_key).await
    }

    /// The entry under `cache_key`, still encoded, so that it can be decoded
    /// off the scheduler's thread.
    async fn get_cache_entry_blob(
        &self,
        cache_key: &CacheEntryKey,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.get_value_blob("cache_entries", cache_key).await
    }

    async fn get_cache_key_by_execution_key(
        &self,
        execution_cache_key: &ExecutionCacheKey,
    ) -> anyhow::Result<Option<CacheEntryKey>> {
        self.get_key_by_value("task_fingerprints", execution_cache_key).await
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "lock guard must be held while executing the prepared statement"
    )]
    async fn upsert<
        K: SchemaWrite<TaskCacheConfig, Src = K>,
        V: SchemaWrite<TaskCacheConfig, Src = V>,
    >(
        &self,
        table: &str,
        key: &K,
        value: &V,
    ) -> anyhow::Result<()> {
        let key_blob = serialize_cache(key)?;
        let value_blob = serialize_cache(value)?;
        let conn = self.conn.lock().await;
        #[expect(clippy::disallowed_macros, reason = "SQL query string for rusqlite requires String")]
        let mut update_stmt = conn.prepare_cached(&format!(
            "INSERT INTO {table} (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=?2"
        ))?;
        update_stmt.execute([key_blob, value_blob])?;
        Ok(())
    }

    async fn upsert_cache_entry(
        &self,
        cache_key: &CacheEntryKey,
        cache_value: &CacheEntryValue,
    ) -> anyhow::Result<()> {
        self.upsert("cache_entries", cache_key, cache_value).await
    }

    async fn upsert_task_fingerprint(
        &self,
        execution_cache_key: &ExecutionCacheKey,
        cache_entry_key: &CacheEntryKey,
    ) -> anyhow::Result<()> {
        self.upsert("task_fingerprints", execution_cache_key, cache_entry_key).await
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "lock guard must be held while iterating over query rows"
    )]
    async fn list_table<
        K: SchemaReadOwned<TaskCacheConfig, Dst = K> + Serialize,
        V: SchemaReadOwned<TaskCacheConfig, Dst = V> + Serialize,
    >(
        &self,
        table: &str,
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        #[expect(
            clippy::disallowed_macros,
            reason = "SQL query string for rusqlite requires String"
        )]
        let mut select_stmt = conn.prepare_cached(&format!("SELECT key, value FROM {table}"))?;
        let mut rows = select_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let key_blob: Vec<u8> = row.get(0)?;
            let value_blob: Vec<u8> = row.get(1)?;
            let key: K = deserialize_cache(&key_blob)?;
            let value: V = deserialize_cache(&value_blob)?;
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
        out.write_all(b"------- task_fingerprints -------\n")?;
        self.list_table::<ExecutionCacheKey, CacheEntryKey>("task_fingerprints", &mut out).await?;
        out.write_all(b"------- cache_entries -------\n")?;
        self.list_table::<CacheEntryKey, CacheEntryValue>("cache_entries", &mut out).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;
    use vt_path::AbsolutePathBuf;

    use super::*;

    fn temp_dir() -> (TempDir, AbsolutePathBuf) {
        let tmp = TempDir::new().unwrap();
        let dir = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        (tmp, dir)
    }

    fn open_raw(db: &AbsolutePath) -> Connection {
        Connection::open(db.as_path()).unwrap()
    }

    /// Reopening the same cache directory keeps existing entries: the tables are
    /// created with `IF NOT EXISTS`, so a second open never wipes the database.
    #[test]
    fn reopening_preserves_existing_entries() {
        let (_tmp, dir) = temp_dir();

        drop(ExecutionCache::load_from_path(&dir).unwrap());
        {
            let conn = open_raw(&dir.join("cache.db"));
            conn.execute("INSERT INTO cache_entries (key, value) VALUES (X'01', X'02')", ())
                .unwrap();
        }

        // Reopening must not recreate or clear the tables.
        drop(ExecutionCache::load_from_path(&dir).unwrap());

        let count: u32 = open_raw(&dir.join("cache.db"))
            .query_one("SELECT COUNT(*) FROM cache_entries", (), |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    /// Regression test for vite-plus#1785: two different schema-version
    /// directories under the same cache base are fully independent, so caches
    /// from different Vite+ versions never collide (each version reads and
    /// writes only its own directory).
    #[test]
    fn version_directories_are_isolated() {
        let (_tmp, base) = temp_dir();

        let dir_a = base.join("v13");
        let dir_b = base.join("v14");

        drop(ExecutionCache::load_from_path(&dir_a).unwrap());
        drop(ExecutionCache::load_from_path(&dir_b).unwrap());

        assert!(dir_a.join("cache.db").as_path().exists());
        assert!(dir_b.join("cache.db").as_path().exists());

        // A row written into A is invisible to B.
        {
            let conn = open_raw(&dir_a.join("cache.db"));
            conn.execute("INSERT INTO cache_entries (key, value) VALUES (X'01', X'02')", ())
                .unwrap();
        }
        let count_b: u32 = open_raw(&dir_b.join("cache.db"))
            .query_one("SELECT COUNT(*) FROM cache_entries", (), |r| r.get(0))
            .unwrap();
        assert_eq!(count_b, 0);
    }

    fn tracked_env_changed(name: &str) -> Change {
        Change::Env {
            mismatch: EnvMismatch::Changed { name: Str::from(name) },
            source: EnvSource::Tracked,
        }
    }

    /// Env names compare by the platform's rules: on Windows, a tool's
    /// `getEnv('probe_env')` and a query that matches `PROBE_ENV` name the
    /// same env, so its change is listed once.
    #[test]
    fn env_changes_dedupe_by_platform_rules() {
        let comparison = Comparison::new(
            vec![
                tracked_env_changed("probe_env"),
                tracked_env_changed("PROBE_ENV"),
                tracked_env_changed("probe_env"),
            ],
            None,
            false,
        );
        assert_eq!(comparison.changes.len(), if cfg!(windows) { 1 } else { 2 });
    }

    /// `synchronous` is `NORMAL` only when the database is in WAL mode (see `enable_wal` for why).
    #[test]
    fn sync_is_relaxed_only_in_wal_mode() {
        let (_tmp, dir) = temp_dir();
        let cache = ExecutionCache::load_from_path(&dir).unwrap();
        let (mode, sync): (Box<str>, _) = {
            let conn = cache.conn.try_lock().unwrap();
            (
                conn.query_one("PRAGMA journal_mode", (), |r| r.get(0)).unwrap(),
                conn.query_one("PRAGMA synchronous", (), |r| r.get(0)).unwrap(),
            )
        };
        assert_eq!((&*mode, sync), ("wal", 1)); // 1 = NORMAL

        // An in-memory database can't use WAL: `journal_mode=WAL` leaves it in `memory` mode,
        // and `synchronous` keeps the SQLite default, FULL.
        let memory = Connection::open_in_memory().unwrap();
        enable_wal(&memory).unwrap();
        let mode: Box<str> = memory.query_one("PRAGMA journal_mode", (), |r| r.get(0)).unwrap();
        let sync = memory.query_one("PRAGMA synchronous", (), |r| r.get(0)).unwrap();
        assert_eq!((&*mode, sync), ("memory", 2)); // 2 = FULL
    }
}

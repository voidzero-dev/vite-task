//! Structured summary types for persisting and rendering execution results.
//!
//! The [`LastRunSummary`] is built after every graph execution and:
//! 1. Saved atomically to `cache_path/last-summary.json` for `vp run --last-details`.
//! 2. Rendered immediately — either as a compact one-liner or a full detailed summary.
//!
//! Both the live reporter and the `--last-details` display use the same rendering
//! functions, ensuring consistent output.

use std::{
    fmt::Display,
    io::Write,
    num::{NonZeroI32, NonZeroUsize},
    time::Duration,
};

use owo_colors::Style;
use serde::{Deserialize, Serialize};
use vt_path::{AbsolutePath, RelativePath};
use vt_str::Str;

use super::{CACHE_MISS_STYLE, COMMAND_STYLE, ColorizeExt};
use crate::session::{
    cache::{
        CacheHitSource, CacheMiss, EnvMismatch, FingerprintMismatch, InputChangeKind,
        SpawnFingerprintChange, detect_spawn_fingerprint_changes, format_input_change_str,
        format_spawn_change,
    },
    event::{CacheDisabledReason, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus},
    execute::fingerprint::TrackedEnvQuery,
};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Structured types (Serialize + Deserialize for JSON persistence)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Saved summary of a task runner execution.
///
/// Persisted as `last-summary.json` in the cache directory.
#[derive(Serialize, Deserialize)]
pub struct LastRunSummary {
    pub tasks: Vec<TaskSummary>,
    pub exit_code: u8,
}

/// Summary for a single task execution.
///
/// All fields are structured data — no pre-formatted display strings.
/// Formatting (including color decisions) happens at render time.
#[derive(Serialize, Deserialize)]
pub struct TaskSummary {
    pub package_name: Str,
    pub task_name: Str,
    /// Raw command text (e.g., "vitest run").
    pub command: Str,
    /// Working directory relative to workspace root (e.g., "packages/lib").
    /// Empty string when the cwd is the workspace root.
    pub cwd: Str,
    /// Combined cache status and execution outcome.
    pub result: TaskResult,
}

/// The complete result of a task execution.
///
/// Encodes both the cache status and execution outcome in a single enum,
/// making invalid combinations unrepresentable.
#[derive(Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "built once per task for the summary; boxing would only add indirection"
)]
pub enum TaskResult {
    /// Cache hit — output was replayed from cache. Always successful.
    CacheHit {
        saved_duration_ms: u64,
        /// Summaries saved before the source was recorded read as local hits.
        #[serde(default)]
        source: CacheHitSource,
    },

    /// Cache hit whose output files couldn't be restored. Always a failure.
    RestoreFailed { source: CacheHitSource, error: SavedError },

    /// In-process execution (built-in command like echo). Always successful.
    InProcess,

    /// A process was spawned.
    Spawned {
        /// Why the process was spawned (cache miss or cache disabled).
        cache_status: SpawnedCacheStatus,
        outcome: SpawnOutcome,
    },
}

/// Cache status for tasks that required spawning a process.
///
/// Only two cache statuses lead to spawning:
/// - `Miss`: cache lookup found no match or a mismatch.
/// - `Disabled`: no cache configuration for this task.
///
/// `Hit` is handled by [`TaskResult::CacheHit`] or [`TaskResult::RestoreFailed`],
/// and `InProcessExecution` by [`TaskResult::InProcess`].
#[derive(Serialize, Deserialize)]
pub enum SpawnedCacheStatus {
    Miss(SavedCacheMissReason),
    /// No cache configuration for this task.
    Disabled,
}

/// Outcome of a spawned process.
#[derive(Serialize, Deserialize)]
pub enum SpawnOutcome {
    /// Process exited successfully (exit code 0).
    /// May have a post-execution infrastructure error (cache update or fingerprint failed).
    /// These only run after exit 0, so this field only exists on the success path.
    Success {
        infra_error: Option<SavedError>,
        /// First path that was both read and written, causing cache to be skipped.
        /// Only set when fspy detected a read-write overlap.
        input_modified: Option<InputModified>,
        /// `true` when the task required fspy auto-inference but the binary was
        /// built without `cfg(fspy)` (e.g., cross-compiled to an unsupported OS).
        /// Task ran successfully but cache was not updated.
        #[serde(default)]
        fspy_unsupported: bool,
        /// The IPC server error that caused the cache to be skipped, if any.
        ipc_server_error: Option<SavedError>,
        /// `true` when the task made more file accesses than tracking had
        /// room for, so the inferred inputs and outputs were a subset of
        /// what it touched. Task ran successfully but cache was not
        /// updated.
        #[serde(default)]
        tracking_incomplete: bool,
        /// Set when a runner-aware tool called `disableCache()`, skipping
        /// cache update.
        tool_disabled_cache: bool,
        /// Why uploading the entry to the remote cache failed, if it did.
        /// The local cache was still updated.
        upload_error: Option<SavedError>,
    },

    /// Process exited with non-zero status.
    /// [`NonZeroI32`] enforces that exit code 0 is unrepresentable here.
    /// No `infra_error` field: cache operations are skipped on non-zero exit.
    Failed { exit_code: NonZeroI32 },

    /// Execution failed without a usable process exit status.
    SpawnError(SavedError),
}

/// A path that a task both read and wrote.
#[derive(Serialize, Deserialize)]
pub struct InputModified {
    /// Relative to the workspace root.
    path: Str,
    /// Relative to the task's package directory, or `None` if the path is
    /// outside it.
    path_in_package: Option<Str>,
}

/// Why a cache miss occurred.
#[derive(Serialize, Deserialize)]
pub enum SavedCacheMissReason {
    /// No previous cache entry for this task.
    NotFound,
    /// Spawn fingerprint changed (command, envs, cwd, etc.).
    SpawnFingerprintChanged(Vec<SpawnFingerprintChange>),
    /// Task configuration changed (`input_config` or `glob_base`).
    ConfigChanged,
    /// An input file or folder changed.
    InputChanged { kind: InputChangeKind, path: Str },
    /// A runner-aware tool reported a tracked env var that changed between runs.
    TrackedEnvChanged(EnvMismatch),
    /// A runner-aware tool reported a tracked bulk env query whose match-set changed
    /// between runs. Carries the first differing entry.
    TrackedEnvQueryChanged { query: TrackedEnvQuery, mismatch: EnvMismatch },
    /// Reading the remote cache failed, and the local cache had no entry.
    RemoteReadFailed(SavedError),
}

/// An error's message and the messages of its causes, outermost first.
///
/// Compact output shows only `message`. The detailed summary also shows each
/// cause on its own line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedError {
    message: Str,
    causes: Vec<Str>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Computed stats (derived from tasks, not persisted)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

struct SummaryStats {
    total: usize,
    /// Cache hits from either cache, including `remote_cache_hits`.
    cache_hits: usize,
    remote_cache_hits: usize,
    cache_misses: usize,
    cache_disabled: usize,
    failed: usize,
    total_saved: Duration,
    /// Display names of tasks that were not cached due to read-write overlap.
    input_modified_task_names: Vec<Str>,
    /// Tasks whose upload to the remote cache failed.
    upload_failures: Vec<UploadFailure>,
}

struct UploadFailure {
    task_name: Str,
    reason: Str,
}

impl SummaryStats {
    fn compute(tasks: &[TaskSummary]) -> Self {
        let mut stats = Self {
            total: tasks.len(),
            cache_hits: 0,
            remote_cache_hits: 0,
            cache_misses: 0,
            cache_disabled: 0,
            failed: 0,
            total_saved: Duration::ZERO,
            input_modified_task_names: Vec::new(),
            upload_failures: Vec::new(),
        };

        for task in tasks {
            match &task.result {
                TaskResult::CacheHit { saved_duration_ms, source } => {
                    stats.cache_hits += 1;
                    if *source == CacheHitSource::Remote {
                        stats.remote_cache_hits += 1;
                    }
                    stats.total_saved += Duration::from_millis(*saved_duration_ms);
                }
                TaskResult::RestoreFailed { .. } => stats.failed += 1,
                TaskResult::InProcess => {
                    stats.cache_disabled += 1;
                }
                TaskResult::Spawned { cache_status, outcome } => {
                    match cache_status {
                        SpawnedCacheStatus::Miss(_) => stats.cache_misses += 1,
                        SpawnedCacheStatus::Disabled => stats.cache_disabled += 1,
                    }
                    match outcome {
                        SpawnOutcome::Success { infra_error: Some(_), .. }
                        | SpawnOutcome::Failed { .. }
                        | SpawnOutcome::SpawnError(_) => stats.failed += 1,
                        SpawnOutcome::Success { input_modified: Some(_), .. } => {
                            stats.input_modified_task_names.push(task.format_task_display());
                        }
                        SpawnOutcome::Success { .. } => {}
                    }
                    if let SpawnOutcome::Success { upload_error: Some(error), .. } = outcome {
                        stats.upload_failures.push(UploadFailure {
                            task_name: task.format_task_display(),
                            reason: error.message.clone(),
                        });
                    }
                }
            }
        }

        stats
    }

    /// Tasks with caching enabled: the denominator of the cache hit rate.
    const fn cacheable(&self) -> usize {
        self.total - self.cache_disabled
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Conversion from live execution data
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

impl SavedError {
    /// Save the messages of `error` and its sources.
    pub fn new(error: &(dyn std::error::Error + 'static)) -> Self {
        Self {
            message: vt_str::format!("{error}"),
            causes: std::iter::successors(error.source(), |&source| source.source())
                .map(|source| vt_str::format!("{source}"))
                .collect(),
        }
    }
}

impl SavedCacheMissReason {
    fn from_cache_miss(cache_miss: &CacheMiss) -> Self {
        match cache_miss {
            CacheMiss::NotFound => Self::NotFound,
            CacheMiss::FingerprintMismatch(mismatch) => match mismatch {
                FingerprintMismatch::SpawnFingerprint { old, new } => {
                    Self::SpawnFingerprintChanged(detect_spawn_fingerprint_changes(old, new))
                }
                FingerprintMismatch::InputConfig | FingerprintMismatch::OutputConfig => {
                    Self::ConfigChanged
                }
                FingerprintMismatch::InputChanged { kind, path } => {
                    Self::InputChanged { kind: *kind, path: Str::from(path.as_str()) }
                }
                FingerprintMismatch::TrackedEnvChanged(mismatch) => {
                    Self::TrackedEnvChanged(mismatch.clone())
                }
                FingerprintMismatch::TrackedEnvQueryChanged { query, mismatch } => {
                    Self::TrackedEnvQueryChanged {
                        query: query.clone(),
                        mismatch: mismatch.clone(),
                    }
                }
            },
            CacheMiss::RemoteReadFailed(error) => {
                Self::RemoteReadFailed(SavedError::new(error.as_ref()))
            }
        }
    }
}

impl TaskResult {
    /// Build a [`TaskResult`] from live execution data.
    ///
    /// `cache_status`: the cache status determined at `start()` time.
    /// `exit_status`: the process exit status, or `None` for cache hit / in-process.
    /// `saved_error`: an optional pre-converted execution error.
    /// `cache_update_status`: the post-execution cache update result.
    /// `package_path`, `workspace_path`: locate a modified input in the task's package.
    pub fn from_execution(
        cache_status: &CacheStatus,
        exit_status: Option<std::process::ExitStatus>,
        saved_error: Option<&SavedError>,
        cache_update_status: &CacheUpdateStatus,
        package_path: &AbsolutePath,
        workspace_path: &AbsolutePath,
    ) -> Self {
        let input_modified = match cache_update_status {
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::InputModified { path }) => {
                Some(InputModified::new(path, package_path, workspace_path))
            }
            _ => None,
        };
        let fspy_unsupported = matches!(
            cache_update_status,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::FspyUnsupported)
        );
        let ipc_server_error = match cache_update_status {
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::IpcServerError(err)) => {
                Some(SavedError::new(err))
            }
            _ => None,
        };
        let tool_disabled_cache = matches!(
            cache_update_status,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::ToolRequested)
        );
        let tracking_incomplete = matches!(
            cache_update_status,
            CacheUpdateStatus::NotUpdated(CacheNotUpdatedReason::TrackingIncomplete)
        );

        match cache_status {
            // The only error a cache hit can have is a failed restore.
            CacheStatus::Hit { replayed_duration, source } => saved_error.map_or_else(
                || Self::CacheHit {
                    saved_duration_ms: duration_to_ms(*replayed_duration),
                    source: *source,
                },
                |error| Self::RestoreFailed { source: *source, error: error.clone() },
            ),
            CacheStatus::Disabled(CacheDisabledReason::InProcessExecution) => Self::InProcess,
            CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata) => Self::Spawned {
                cache_status: SpawnedCacheStatus::Disabled,
                outcome: spawn_outcome_from_execution(
                    exit_status,
                    saved_error,
                    input_modified,
                    fspy_unsupported,
                    ipc_server_error,
                    tool_disabled_cache,
                    tracking_incomplete,
                ),
            },
            CacheStatus::Miss(cache_miss) => Self::Spawned {
                cache_status: SpawnedCacheStatus::Miss(SavedCacheMissReason::from_cache_miss(
                    cache_miss,
                )),
                outcome: spawn_outcome_from_execution(
                    exit_status,
                    saved_error,
                    input_modified,
                    fspy_unsupported,
                    ipc_server_error,
                    tool_disabled_cache,
                    tracking_incomplete,
                ),
            },
        }
    }

    /// Record why uploading the entry to the remote cache failed. The upload
    /// can fail after the task finishes, so this is set after
    /// [`Self::from_execution`]. Only a successful spawned task uploads an
    /// entry, so other results are left as they are.
    pub fn set_upload_error(&mut self, error: SavedError) {
        if let Self::Spawned { outcome: SpawnOutcome::Success { upload_error, .. }, .. } = self {
            *upload_error = Some(error);
        }
    }
}

/// Build a [`SpawnOutcome`] from process exit status and optional pre-converted error.
/// A failed upload is set later, with [`TaskResult::set_upload_error`].
fn spawn_outcome_from_execution(
    exit_status: Option<std::process::ExitStatus>,
    saved_error: Option<&SavedError>,
    input_modified: Option<InputModified>,
    fspy_unsupported: bool,
    ipc_server_error: Option<SavedError>,
    tool_disabled_cache: bool,
    tracking_incomplete: bool,
) -> SpawnOutcome {
    match (exit_status, saved_error) {
        // Spawn error — process never ran
        (None, Some(err)) => SpawnOutcome::SpawnError(err.clone()),
        // Process exited successfully, possible infra error
        (Some(status), _) if status.success() => SpawnOutcome::Success {
            infra_error: saved_error.cloned(),
            input_modified,
            fspy_unsupported,
            ipc_server_error,
            tool_disabled_cache,
            tracking_incomplete,
            upload_error: None,
        },
        // Process exited with non-zero code
        (Some(status), _) => {
            let code = crate::session::event::exit_status_to_code(status);
            SpawnOutcome::Failed {
                // exit_status_to_code returns 1..=255 for failed processes (see its
                // implementation: always positive, non-zero for non-success status).
                // NonZeroI32::new returns None only for 0, which cannot happen here.
                exit_code: NonZeroI32::new(code).unwrap_or(NonZeroI32::MIN),
            }
        }
        // No exit status, no error — this is the cache hit / in-process path,
        // handled by TaskResult::CacheHit / InProcess before reaching here.
        // If we somehow get here, treat as success.
        (None, None) => SpawnOutcome::Success {
            infra_error: None,
            input_modified: None,
            fspy_unsupported: false,
            ipc_server_error: None,
            tool_disabled_cache: false,
            tracking_incomplete: false,
            upload_error: None,
        },
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "value is clamped to u64::MAX before casting, so no data loss"
)]
fn duration_to_ms(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

fn format_summary_duration(d: Duration) -> Str {
    let formatted = vt_str::format!("{d:.2?}");

    for (suffix, unit) in
        [(".00ms", "ms"), (".00s", "s"), (".00us", "us"), (".00µs", "µs"), (".00ns", "ns")]
    {
        if let Some(prefix) = formatted.as_str().strip_suffix(suffix) {
            return vt_str::format!("{prefix}{unit}");
        }
    }

    formatted
}

impl LastRunSummary {
    // ── Persistence ──────────────────────────────────────────────────────

    /// Write the summary as JSON atomically (write to `.tmp`, then rename).
    ///
    /// Errors are returned to the caller (the reporter logs them without propagating).
    #[expect(
        clippy::disallowed_types,
        reason = "PathBuf is needed to construct a temporary path by appending .tmp suffix; \
                  AbsolutePathBuf cannot be constructed without validation"
    )]
    pub fn write_atomic(&self, path: &AbsolutePath) -> std::io::Result<()> {
        let json = serde_json::to_vec(self).map_err(std::io::Error::other)?;

        let mut tmp_os = path.as_path().as_os_str().to_owned();
        tmp_os.push(".tmp");
        let tmp_path = std::path::PathBuf::from(tmp_os);
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Read a summary from a JSON file.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` on parse or IO errors (caller provides version mismatch message).
    pub fn read_from_path(path: &AbsolutePath) -> Result<Option<Self>, ReadSummaryError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(ReadSummaryError::Io(err)),
        };
        let summary =
            serde_json::from_slice(&bytes).map_err(|_| ReadSummaryError::IncompatibleVersion)?;
        Ok(Some(summary))
    }
}

/// Error type for [`LastRunSummary::read_from_path`].
pub enum ReadSummaryError {
    Io(std::io::Error),
    /// The JSON could not be parsed — likely saved by a different version.
    IncompatibleVersion,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Display helpers for TaskResult
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

impl TaskSummary {
    /// Format the task display name (e.g., "package#task" or "task").
    fn format_task_display(&self) -> Str {
        if self.package_name.is_empty() {
            self.task_name.clone()
        } else {
            vt_str::format!("{}#{}", self.package_name, self.task_name)
        }
    }

    /// Format the command with cwd prefix (e.g., "~/packages/lib$ vitest run").
    fn format_command_display(&self) -> Str {
        if self.cwd.is_empty() {
            vt_str::format!("$ {}", self.command)
        } else {
            vt_str::format!("~/{cwd}$ {cmd}", cwd = self.cwd, cmd = self.command)
        }
    }
}

impl TaskResult {
    /// Whether this task succeeded (used for exit icon rendering).
    const fn is_success(&self) -> bool {
        match self {
            Self::CacheHit { .. } | Self::InProcess => true,
            Self::RestoreFailed { .. } => false,
            Self::Spawned { outcome, .. } => matches!(outcome, SpawnOutcome::Success { .. }),
        }
    }

    /// Format the cache status detail line for the full summary, with the
    /// causes to show below it. Only a remote read failure has causes. The
    /// caller shows [`Self::ipc_server_error`] instead, if there is one.
    ///
    /// Examples:
    /// - "→ Cache hit - output replayed - 102.96ms saved"
    /// - "→ Remote cache hit - output replayed - 102.96ms saved"
    /// - "→ Cache hit, but the outputs couldn't be restored"
    /// - "→ Cache miss: no previous cache entry found"
    /// - "→ Cache disabled in task configuration"
    fn format_cache_detail(&self) -> (Str, &[Str]) {
        // Tool-reported cache disable — the tool said it shouldn't be cached.
        if let Self::Spawned {
            outcome: SpawnOutcome::Success { tool_disabled_cache: true, .. },
            ..
        } = self
        {
            return (Str::from("→ Not cached: the task opted out of caching"), &[]);
        }

        // Check for input modification next — it overrides the cache miss reason.
        // The caller shows how to exclude the path below this line.
        if let Some(InputModified { path, .. }) = self.input_modified() {
            return (vt_str::format!("→ Not cached: the task read and wrote '{path}'"), &[]);
        }
        // Tracking came up short, so the inferred inputs and outputs would
        // have been a subset of what the task touched.
        if let Self::Spawned {
            outcome: SpawnOutcome::Success { tracking_incomplete: true, .. },
            ..
        } = self
        {
            return (
                Str::from(
                    "→ Not cached: this task used more files than automatic tracking can record. Configure `input` and `output` manually to enable caching.",
                ),
                &[],
            );
        }
        // fspy-unsupported-on-this-OS message — same overrides precedence as above
        if let Self::Spawned {
            outcome: SpawnOutcome::Success { fspy_unsupported: true, .. }, ..
        } = self
        {
            return (
                Str::from(
                    "→ Not cached: `input` auto-inference isn't supported on this OS. Configure `input` manually to enable caching.",
                ),
                &[],
            );
        }

        let detail = match self {
            Self::CacheHit { saved_duration_ms, source } => {
                let d = Duration::from_millis(*saved_duration_ms);
                let formatted_duration = format_summary_duration(d);
                let hit = format_hit(*source);
                vt_str::format!("→ {hit} - output replayed - {formatted_duration} saved")
            }
            Self::RestoreFailed { source, .. } => {
                vt_str::format!("→ {}, but the outputs couldn't be restored", format_hit(*source))
            }
            Self::InProcess => Str::from("→ Cache disabled for built-in command"),
            Self::Spawned { cache_status, .. } => match cache_status {
                SpawnedCacheStatus::Disabled => Str::from("→ Cache disabled in task configuration"),
                SpawnedCacheStatus::Miss(reason) => match reason {
                    SavedCacheMissReason::NotFound => {
                        Str::from("→ Cache miss: no previous cache entry found")
                    }
                    SavedCacheMissReason::SpawnFingerprintChanged(changes) => {
                        let formatted: Vec<Str> = changes.iter().map(format_spawn_change).collect();
                        if formatted.is_empty() {
                            Str::from("→ Cache miss: configuration changed")
                        } else {
                            let joined =
                                formatted.iter().map(Str::as_str).collect::<Vec<_>>().join("; ");
                            vt_str::format!("→ Cache miss: {joined}")
                        }
                    }
                    SavedCacheMissReason::ConfigChanged => {
                        Str::from("→ Cache miss: input configuration changed")
                    }
                    SavedCacheMissReason::InputChanged { kind, path } => {
                        let desc = format_input_change_str(*kind, path.as_str());
                        vt_str::format!("→ Cache miss: {desc}")
                    }
                    SavedCacheMissReason::TrackedEnvChanged(mismatch)
                    | SavedCacheMissReason::TrackedEnvQueryChanged { mismatch, .. } => {
                        vt_str::format!("→ Cache miss: {mismatch}")
                    }
                    SavedCacheMissReason::RemoteReadFailed(error) => {
                        return (vt_str::format!("→ Cache miss: {}", error.message), &error.causes);
                    }
                },
            },
        };
        (detail, &[])
    }

    /// The [`Style`] for the cache detail line.
    const fn cache_detail_style(&self) -> Style {
        match self {
            Self::CacheHit { .. } => Style::new().green(),
            Self::RestoreFailed { .. } => Style::new().red(),
            Self::InProcess => Style::new().bright_black(),
            Self::Spawned { cache_status: SpawnedCacheStatus::Disabled, .. } => {
                Style::new().bright_black()
            }
            Self::Spawned { cache_status: SpawnedCacheStatus::Miss(_), .. } => CACHE_MISS_STYLE,
        }
    }

    /// The path the task both read and wrote, which kept it from being cached.
    const fn input_modified(&self) -> Option<&InputModified> {
        match self {
            Self::Spawned { outcome: SpawnOutcome::Success { input_modified, .. }, .. } => {
                input_modified.as_ref()
            }
            _ => None,
        }
    }

    /// The IPC server error that caused the cache to be skipped, if any.
    const fn ipc_server_error(&self) -> Option<&SavedError> {
        match self {
            Self::Spawned { outcome: SpawnOutcome::Success { ipc_server_error, .. }, .. } => {
                ipc_server_error.as_ref()
            }
            _ => None,
        }
    }

    /// Why uploading the entry to the remote cache failed, if it did.
    const fn upload_error(&self) -> Option<&SavedError> {
        match self {
            Self::Spawned { outcome: SpawnOutcome::Success { upload_error, .. }, .. } => {
                upload_error.as_ref()
            }
            _ => None,
        }
    }

    /// Optional error associated with this result.
    pub const fn error(&self) -> Option<&SavedError> {
        match self {
            Self::CacheHit { .. } | Self::InProcess => None,
            Self::RestoreFailed { error, .. } => Some(error),
            Self::Spawned { outcome, .. } => match outcome {
                SpawnOutcome::Success { infra_error, .. } => infra_error.as_ref(),
                SpawnOutcome::Failed { .. } => None,
                SpawnOutcome::SpawnError(err) => Some(err),
            },
        }
    }
}

/// "Cache hit" or "Remote cache hit", for the full summary's detail line.
const fn format_hit(source: CacheHitSource) -> &'static str {
    match source {
        CacheHitSource::Local => "Cache hit",
        CacheHitSource::Remote => "Remote cache hit",
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Full summary rendering (--verbose and --last-details)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Render the full detailed execution summary.
///
/// Used by both `--verbose` (live) and `--last-details` (from file).
#[expect(
    clippy::too_many_lines,
    reason = "summary formatting is inherently verbose with many write calls"
)]
pub fn format_full_summary(summary: &LastRunSummary) -> Vec<u8> {
    let mut buf = Vec::new();

    let stats = SummaryStats::compute(&summary.tasks);

    // Header
    let _ = writeln!(buf);
    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(
        buf,
        "{}",
        "    Vite+ Task Runner • Execution Summary".style(Style::new().bold().bright_white())
    );
    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );
    let _ = writeln!(buf);

    // Statistics line
    let cache_disabled_str = if stats.cache_disabled > 0 {
        let n = stats.cache_disabled;
        Str::from(
            vt_str::format!("• {n} cache disabled").style(Style::new().bright_black()).to_string(),
        )
    } else {
        Str::default()
    };

    let failed_str = if stats.failed > 0 {
        let n = stats.failed;
        Str::from(vt_str::format!("• {n} failed").style(Style::new().red()).to_string())
    } else {
        Str::default()
    };

    let total = stats.total;
    let cache_hits = stats.cache_hits;
    let cache_hits_count = count_noun(cache_hits, "cache hit", "cache hits");
    let cache_hits_str = match stats.remote_cache_hits {
        0 => vt_str::format!("• {cache_hits_count}"),
        remote => vt_str::format!("• {cache_hits_count} ({remote} remote)"),
    };
    let _ = write!(
        buf,
        "{}  {} {} {}",
        "Statistics:".style(Style::new().bold()),
        vt_str::format!(" {}", count_noun(total, "task", "tasks"))
            .style(Style::new().bright_white()),
        cache_hits_str.style(Style::new().green()),
        vt_str::format!("• {}", count_noun(stats.cache_misses, "cache miss", "cache misses"))
            .style(CACHE_MISS_STYLE),
    );
    if !cache_disabled_str.is_empty() {
        let _ = write!(buf, " {cache_disabled_str}");
    }
    if !failed_str.is_empty() {
        let _ = write!(buf, " {failed_str}");
    }
    let _ = writeln!(buf);

    // Performance line
    let cacheable = stats.cacheable();
    let _ = write!(buf, "{}  ", "Performance:".style(Style::new().bold()));
    if cacheable == 0 {
        let _ = write!(buf, "{}", "no task has caching enabled".style(Style::new().bright_black()));
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "percentage is always 0..=100, fits in u32"
        )]
        #[expect(clippy::cast_sign_loss, reason = "percentage is always non-negative")]
        #[expect(
            clippy::cast_precision_loss,
            reason = "acceptable precision loss for display percentage"
        )]
        let cache_rate = (cache_hits as f64 / cacheable as f64 * 100.0) as u32;

        let _ = write!(
            buf,
            "{} cache hit rate",
            format_args!("{cache_rate}%").style(if cache_rate >= 75 {
                Style::new().green().bold()
            } else if cache_rate >= 50 {
                CACHE_MISS_STYLE
            } else {
                Style::new().red()
            })
        );
    }

    if stats.total_saved > Duration::ZERO {
        let formatted_total_saved = format_summary_duration(stats.total_saved);
        let _ = write!(
            buf,
            ", {} saved in total",
            formatted_total_saved.style(Style::new().green().bold())
        );
    }
    let _ = writeln!(buf);
    let _ = writeln!(buf);

    // Task Details
    let _ = writeln!(buf, "{}", "Task Details:".style(Style::new().bold()));
    let _ = writeln!(
        buf,
        "{}",
        "────────────────────────────────────────────────".style(Style::new().bright_black())
    );

    for (idx, task) in summary.tasks.iter().enumerate() {
        // Task index and name
        let _ = write!(
            buf,
            "  {} {}",
            vt_str::format!("[{}]", idx + 1).style(Style::new().bright_black()),
            task.format_task_display().to_string().style(Style::new().bright_white().bold())
        );

        // Command with cwd prefix
        let _ = write!(buf, ": {}", task.format_command_display().style(COMMAND_STYLE));

        // Exit icon
        if task.result.is_success() {
            let _ = write!(buf, " {}", "✓".style(Style::new().green().bold()));
        } else if let TaskResult::Spawned { outcome: SpawnOutcome::Failed { exit_code }, .. } =
            &task.result
        {
            let code = exit_code.get();
            let _ = write!(
                buf,
                " {} {}",
                "✗".style(Style::new().red().bold()),
                vt_str::format!("(exit code: {code})").style(Style::new().red())
            );
        }
        let _ = writeln!(buf);

        // Cache status detail. An IPC server error short-circuits before any
        // cache computation in `execute_spawn`, so it takes priority.
        let detail_style = task.result.cache_detail_style();
        if let Some(error) = task.result.ipc_server_error() {
            write_error_lines(
                &mut buf,
                "→ Not cached: task communication failed:".style(detail_style),
                error,
                detail_style,
            );
        } else {
            let (cache_detail, causes) = task.result.format_cache_detail();
            let _ = writeln!(buf, "      {}", cache_detail.style(detail_style));
            write_causes(&mut buf, causes, detail_style);
            if let Some(entry) = task.result.input_modified().and_then(InputModified::exclude_entry)
            {
                write_input_modified_hint(&mut buf, &entry);
            }
        }

        if let Some(error) = task.result.upload_error() {
            let style = Style::new().yellow();
            write_error_lines(
                &mut buf,
                "⚠ Not uploaded to the remote cache:".style(style),
                error,
                style,
            );
        }

        if let Some(error) = task.result.error() {
            write_error_lines(
                &mut buf,
                "✗ Error:".style(Style::new().red().bold()),
                error,
                Style::new().red(),
            );
        }

        // Separator between tasks (except last)
        if idx < summary.tasks.len() - 1 {
            let _ = writeln!(
                buf,
                "  {}",
                "·······················································"
                    .style(Style::new().bright_black())
            );
        }
    }

    let _ = writeln!(
        buf,
        "{}",
        "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━".style(Style::new().bright_black())
    );

    buf
}

/// Write a task detail line with `label` and `error`'s message, then each of
/// its causes on its own line below.
fn write_error_lines(buf: &mut Vec<u8>, label: impl Display, error: &SavedError, style: Style) {
    let _ = writeln!(buf, "      {label} {}", error.message.style(style));
    write_causes(buf, &error.causes, style);
}

/// Format `count` followed by `singular` if it is 1, or by `plural` otherwise.
fn count_noun(count: usize, singular: &str, plural: &str) -> Str {
    vt_str::format!("{count} {}", if count == 1 { singular } else { plural })
}

/// Write each cause on its own line, below a task detail line.
fn write_causes(buf: &mut Vec<u8>, causes: &[Str], style: Style) {
    for cause in causes {
        let _ = writeln!(buf, "        {}", vt_str::format!("↳ {cause}").style(style));
    }
}

/// Write the `cache` settings that exclude a path the task read and wrote,
/// below a task detail line. `entry` is from [`InputModified::exclude_entry`].
/// Both lists need `{ auto: true }`: without it, a list of exclusions alone
/// would turn off automatic tracking.
fn write_input_modified_hint(buf: &mut Vec<u8>, entry: &str) {
    let _ = writeln!(
        buf,
        "        {}",
        "If this file is temporary or shouldn't affect caching, exclude it (or a glob matching it) in the task's `cache` config:"
            .style(Style::new().bright_black())
    );
    for field in ["input", "output"] {
        let _ = writeln!(
            buf,
            "          {}",
            vt_str::format!("{field}: [{{ auto: true }}, {entry}],").style(COMMAND_STYLE)
        );
    }
}

impl InputModified {
    /// `path` is relative to `workspace_path`.
    fn new(
        path: &RelativePath,
        package_path: &AbsolutePath,
        workspace_path: &AbsolutePath,
    ) -> Self {
        let path_in_package =
            package_path.strip_prefix(workspace_path).ok().flatten().and_then(|package_dir| {
                path.strip_prefix(&package_dir).map(|p| Str::from(p.as_str()))
            });
        Self { path: Str::from(path.as_str()), path_in_package }
    }

    /// The `input`/`output` entry that excludes this path, written as a JS
    /// value. Paths outside the package, and the package directory itself,
    /// need the workspace as their base.
    ///
    /// `None` for the workspace root, which a task reads and writes when it
    /// opens the root directory for both. The empty pattern for it would
    /// resolve to `**` and exclude every file.
    fn exclude_entry(&self) -> Option<Str> {
        // `serde_json` quotes the pattern as a string literal that is also valid JS.
        let quote = |path: &str| {
            let pattern = vt_str::format!("!{}", wax::escape(path));
            vt_str::format!("{}", serde_json::Value::from(pattern.as_str()))
        };
        match self.path_in_package.as_deref() {
            Some(path) if !path.is_empty() => Some(quote(path)),
            _ if self.path.is_empty() => None,
            _ => Some(vt_str::format!("{{ pattern: {}, base: \"workspace\" }}", quote(&self.path))),
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Compact summary rendering (default mode)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Render a compact summary (one-liner or empty).
///
/// Rules:
/// - No tasks → empty
/// - Single task + not cache hit → empty (no summary at all)
/// - Single task + cache hit → thin line + "vp run: cache hit, {duration} saved."
///   ("remote cache hit" for a remote hit)
/// - Multi-task → thin line + "vp run: {hits}/{cacheable} cache hit ({rate}%),
///   {successful}/{total} successful, {duration} saved." where `cacheable` leaves out tasks
///   with caching disabled, and `successful` includes cache hits. The hit count is left out
///   when `cacheable` is 0, and has an optional remote hit count ("({rate}%, {remote} remote)").
///   Followed by the `--last-details` hint.
pub fn format_compact_summary(summary: &LastRunSummary, program_name: &str) -> Vec<u8> {
    if summary.tasks.is_empty() {
        return Vec::new();
    }

    let stats = SummaryStats::compute(&summary.tasks);

    let is_single_task = summary.tasks.len() == 1;

    // Single task + not cache hit + no notice → no summary
    if is_single_task
        && stats.cache_hits == 0
        && stats.input_modified_task_names.is_empty()
        && stats.upload_failures.is_empty()
    {
        return Vec::new();
    }

    let mut buf = Vec::new();

    // Thin line separator
    let _ = writeln!(buf, "{}", "---".style(Style::new().bright_black()));

    let run_label = vt_str::format!("{program_name} run:");
    let mut show_last_details_hint = true;
    if is_single_task && stats.cache_hits > 0 {
        // Single task cache hit — no need for --last-details hint
        let formatted_total_saved = format_summary_duration(stats.total_saved);
        let hit = if stats.remote_cache_hits > 0 { "remote cache hit" } else { "cache hit" };
        let _ = write!(
            buf,
            "{} {hit}, {} saved.",
            run_label.as_str().style(Style::new().blue().bold()),
            formatted_total_saved.style(Style::new().green().bold()),
        );
        show_last_details_hint = false;
    } else if !is_single_task {
        // Multi-task
        let _ = write!(buf, "{}", run_label.as_str().style(Style::new().blue().bold()));
        let cacheable = stats.cacheable();

        // No hit count when no task had caching enabled
        if cacheable > 0 {
            let hits = stats.cache_hits;

            #[expect(
                clippy::cast_possible_truncation,
                reason = "percentage is always 0..=100, fits in u32"
            )]
            #[expect(clippy::cast_sign_loss, reason = "percentage is always non-negative")]
            #[expect(
                clippy::cast_precision_loss,
                reason = "acceptable precision loss for display percentage"
            )]
            let rate = (hits as f64 / cacheable as f64 * 100.0) as u32;

            let _ = write!(buf, " {hits}/{cacheable} cache hit ({rate}%");
            if stats.remote_cache_hits > 0 {
                let _ = write!(buf, ", {} remote", stats.remote_cache_hits);
            }
            let _ = write!(buf, "),");
        }

        let total = stats.total;
        let successful = total - stats.failed;
        let successful_style = if successful < total { Style::new().red() } else { Style::new() };
        let _ = write!(
            buf,
            " {} successful",
            vt_str::format!("{successful}/{total}").style(successful_style)
        );

        if stats.total_saved > Duration::ZERO {
            let formatted_total_saved = format_summary_duration(stats.total_saved);
            let _ =
                write!(buf, ", {} saved", formatted_total_saved.style(Style::new().green().bold()));
        }

        let _ = write!(buf, ".");
    } else {
        // Single task, no cache hit — only shown with a notice below
        let _ = write!(buf, "{}", run_label.as_str().style(Style::new().blue().bold()));
    }

    // Inline notices before the --last-details hint
    if !stats.input_modified_task_names.is_empty() {
        format_input_modified_notice(&mut buf, &stats.input_modified_task_names);
    }
    if !stats.upload_failures.is_empty() {
        format_upload_failed_notice(&mut buf, &stats.upload_failures);
    }

    if show_last_details_hint {
        let last_details_cmd = vt_str::format!("`{program_name} run --last-details`");
        let _ = write!(buf, " {}", "(Run ".style(Style::new().bright_black()));
        let _ = write!(buf, "{}", last_details_cmd.as_str().style(COMMAND_STYLE));
        let _ = write!(buf, "{}", " for full details)".style(Style::new().bright_black()));
    }
    let _ = writeln!(buf);

    buf
}

/// Write the "not cached because it modified its inputs" notice inline.
fn format_input_modified_notice(buf: &mut Vec<u8>, task_names: &[Str]) {
    let _ = write!(buf, " ");

    let first = &task_names[0];
    let _ = write!(buf, "{}", first.as_str().style(Style::new().bold()));
    let remaining = task_names.len() - 1;
    if remaining > 0 {
        let _ = write!(buf, " (and {remaining} more)");
    }

    if task_names.len() == 1 {
        let _ = write!(buf, " not cached because it modified its inputs.");
    } else {
        let _ = write!(buf, " not cached because they modified their inputs.");
    }
}

/// Write the "not uploaded to the remote cache" notice inline. The reason is
/// shown when all failed uploads share it.
fn format_upload_failed_notice(buf: &mut Vec<u8>, failures: &[UploadFailure]) {
    let _ = write!(buf, " ");

    let first = &failures[0];
    let _ = write!(buf, "{}", first.task_name.as_str().style(Style::new().bold()));
    let remaining = failures.len() - 1;
    if remaining > 0 {
        let _ = write!(buf, " (and {remaining} more)");
    }

    let _ = write!(buf, " not uploaded to the remote cache");
    if failures.iter().all(|failure| failure.reason == first.reason) {
        let _ = write!(buf, ": {}", first.reason);
    }
    let _ = write!(buf, ".");
}

/// Render the line shown when all tasks are done, but `count` uploads to the
/// remote cache are still running.
pub fn format_uploads_pending(count: NonZeroUsize) -> Vec<u8> {
    let uploads = if count.get() == 1 { "upload" } else { "uploads" };
    let mut buf = Vec::new();
    let _ = writeln!(
        buf,
        "{}",
        vt_str::format!("Waiting for {count} remote cache {uploads} to finish...")
            .style(Style::new().bright_black())
    );
    buf
}

#[cfg(test)]
mod tests {
    use vt_path::RelativePathBuf;

    use super::*;
    use crate::session::event::ExecutionError;

    fn saved_error(message: &str, causes: &[&str]) -> SavedError {
        SavedError {
            message: Str::from(message),
            causes: causes.iter().copied().map(Str::from).collect(),
        }
    }

    fn upload_failed_task(task_name: &str, error: SavedError) -> TaskSummary {
        TaskSummary {
            package_name: Str::from("pkg"),
            task_name: Str::from(task_name),
            command: Str::from("build"),
            cwd: Str::default(),
            result: TaskResult::Spawned {
                cache_status: SpawnedCacheStatus::Miss(SavedCacheMissReason::NotFound),
                outcome: SpawnOutcome::Success {
                    infra_error: None,
                    input_modified: None,
                    fspy_unsupported: false,
                    ipc_server_error: None,
                    tracking_incomplete: false,
                    tool_disabled_cache: false,
                    upload_error: Some(error),
                },
            },
        }
    }

    fn strip(bytes: &[u8]) -> Str {
        vt_str::format!("{}", anstream::adapter::strip_str(std::str::from_utf8(bytes).unwrap()))
    }

    fn cache_hit_task(task_name: &str, source: CacheHitSource) -> TaskSummary {
        TaskSummary {
            package_name: Str::from("pkg"),
            task_name: Str::from(task_name),
            command: Str::from("build"),
            cwd: Str::default(),
            result: TaskResult::CacheHit { saved_duration_ms: 1000, source },
        }
    }

    fn cache_miss_task(task_name: &str) -> TaskSummary {
        TaskSummary {
            package_name: Str::from("pkg"),
            task_name: Str::from(task_name),
            command: Str::from("build"),
            cwd: Str::default(),
            result: TaskResult::Spawned {
                cache_status: SpawnedCacheStatus::Miss(SavedCacheMissReason::NotFound),
                outcome: SpawnOutcome::Success {
                    infra_error: None,
                    input_modified: None,
                    fspy_unsupported: false,
                    ipc_server_error: None,
                    tracking_incomplete: false,
                    tool_disabled_cache: false,
                    upload_error: None,
                },
            },
        }
    }

    fn cache_disabled_task(task_name: &str) -> TaskSummary {
        let mut task = cache_miss_task(task_name);
        if let TaskResult::Spawned { cache_status, .. } = &mut task.result {
            *cache_status = SpawnedCacheStatus::Disabled;
        }
        task
    }

    /// A task in the package at `package_dir` that read and wrote `path`, both
    /// relative to the workspace root.
    fn input_modified_task(path: &str, package_dir: &str) -> TaskSummary {
        #[cfg(unix)]
        let workspace = AbsolutePath::new("/ws").unwrap();
        #[cfg(windows)]
        let workspace = AbsolutePath::new(r"C:\ws").unwrap();
        let package = if package_dir.is_empty() {
            workspace.to_absolute_path_buf()
        } else {
            workspace.join(package_dir)
        };
        let mut task = cache_miss_task("a");
        if let TaskResult::Spawned {
            outcome: SpawnOutcome::Success { input_modified, .. }, ..
        } = &mut task.result
        {
            *input_modified =
                Some(InputModified::new(&RelativePathBuf::new(path).unwrap(), &package, workspace));
        }
        task
    }

    fn compact_summary(tasks: Vec<TaskSummary>) -> Str {
        strip(&format_compact_summary(&LastRunSummary { tasks, exit_code: 0 }, "vp"))
    }

    fn full_summary(tasks: Vec<TaskSummary>) -> Str {
        strip(&format_full_summary(&LastRunSummary { tasks, exit_code: 0 }))
    }

    #[test]
    fn compact_summary_says_a_task_modified_its_inputs() {
        assert_eq!(
            compact_summary(vec![input_modified_task("src/data.txt", "")]).as_str(),
            "---\nvp run: pkg#a not cached because it modified its inputs. \
             (Run `vp run --last-details` for full details)\n"
        );
    }

    #[test]
    fn full_summary_shows_how_to_exclude_a_modified_input() {
        let summary =
            full_summary(vec![input_modified_task("packages/a/src/data.txt", "packages/a")]);
        assert!(
            summary.as_str().contains(
                "\n      → Not cached: the task read and wrote 'packages/a/src/data.txt'\n        \
                 If this file is temporary or shouldn't affect caching, exclude it (or a glob \
                 matching it) in the task's `cache` config:\n          \
                 input: [{ auto: true }, \"!src/data.txt\"],\n          \
                 output: [{ auto: true }, \"!src/data.txt\"],\n"
            ),
            "{summary}"
        );
    }

    #[test]
    fn modified_input_outside_the_package_is_excluded_from_the_workspace() {
        let summary =
            full_summary(vec![input_modified_task("node_modules/.cache/x", "packages/a")]);
        assert!(
            summary.as_str().contains(
                "\n          input: [{ auto: true }, \
                 { pattern: \"!node_modules/.cache/x\", base: \"workspace\" }],\n"
            ),
            "{summary}"
        );
    }

    #[test]
    fn modified_package_directory_is_excluded_from_the_workspace() {
        let summary = full_summary(vec![input_modified_task("packages/a", "packages/a")]);
        assert!(
            summary.as_str().contains(
                "\n          input: [{ auto: true }, \
                 { pattern: \"!packages/a\", base: \"workspace\" }],\n"
            ),
            "{summary}"
        );
    }

    /// An empty pattern would resolve to `**` and exclude every file.
    #[test]
    fn modified_workspace_root_has_no_exclusion() {
        for package_dir in ["", "packages/a"] {
            let summary = full_summary(vec![input_modified_task("", package_dir)]);
            assert!(summary.as_str().contains("→ Not cached: the task read and wrote ''\n"));
            assert!(!summary.as_str().contains("exclude it"), "{summary}");
            assert!(!summary.as_str().contains("auto: true"), "{summary}");
        }
    }

    #[test]
    fn modified_input_exclusion_is_escaped_and_quoted() {
        let summary = full_summary(vec![input_modified_task("app/[id]/\"x\".ts", "")]);
        assert!(
            summary.as_str().contains(r#"input: [{ auto: true }, "!app/\\[id\\]/\"x\".ts"],"#),
            "{summary}"
        );
    }

    #[test]
    fn compact_summary_names_remote_hits() {
        assert_eq!(
            compact_summary(vec![cache_hit_task("a", CacheHitSource::Remote)]).as_str(),
            "---\nvp run: remote cache hit, 1s saved.\n"
        );
        assert_eq!(
            compact_summary(vec![cache_hit_task("a", CacheHitSource::Local)]).as_str(),
            "---\nvp run: cache hit, 1s saved.\n"
        );
        assert_eq!(
            compact_summary(vec![
                cache_hit_task("a", CacheHitSource::Local),
                cache_hit_task("b", CacheHitSource::Remote),
                cache_miss_task("c"),
            ])
            .as_str(),
            "---\nvp run: 2/3 cache hit (66%, 1 remote), 3/3 successful, 2s saved. \
             (Run `vp run --last-details` for full details)\n"
        );
        assert_eq!(
            compact_summary(vec![cache_hit_task("a", CacheHitSource::Local), cache_miss_task("b")])
                .as_str(),
            "---\nvp run: 1/2 cache hit (50%), 2/2 successful, 1s saved. \
             (Run `vp run --last-details` for full details)\n"
        );
    }

    #[test]
    fn cache_hit_rate_leaves_out_tasks_with_caching_disabled() {
        let tasks = || {
            vec![
                cache_hit_task("a", CacheHitSource::Local),
                cache_miss_task("b"),
                cache_disabled_task("c"),
            ]
        };
        assert_eq!(
            compact_summary(tasks()).as_str(),
            "---\nvp run: 1/2 cache hit (50%), 3/3 successful, 1s saved. \
             (Run `vp run --last-details` for full details)\n"
        );
        assert!(
            full_summary(tasks())
                .as_str()
                .lines()
                .any(|line| line == "Performance:  50% cache hit rate, 1s saved in total")
        );
        assert_eq!(
            compact_summary(vec![cache_disabled_task("a"), cache_disabled_task("b")]).as_str(),
            "---\nvp run: 2/2 successful. (Run `vp run --last-details` for full details)\n"
        );
        assert!(
            full_summary(vec![cache_disabled_task("a")])
                .as_str()
                .lines()
                .any(|line| line == "Performance:  no task has caching enabled")
        );
    }

    #[test]
    fn full_summary_names_remote_hits() {
        let summary = full_summary(vec![
            cache_hit_task("a", CacheHitSource::Local),
            cache_hit_task("b", CacheHitSource::Remote),
            cache_miss_task("c"),
        ]);
        let lines: Vec<&str> = summary.as_str().lines().collect();
        assert!(lines.contains(&"Statistics:   3 tasks • 2 cache hits (1 remote) • 1 cache miss"));
        assert!(lines.contains(&"      → Cache hit - output replayed - 1s saved"));
        assert!(lines.contains(&"      → Remote cache hit - output replayed - 1s saved"));

        let summary = full_summary(vec![cache_hit_task("a", CacheHitSource::Local)]);
        assert!(
            summary
                .as_str()
                .lines()
                .any(|line| line == "Statistics:   1 task • 1 cache hit • 0 cache misses")
        );
    }

    #[test]
    fn saved_cache_hit_without_source_is_local() {
        let summary: LastRunSummary = serde_json::from_str(
            r#"{"tasks":[{"package_name":"pkg","task_name":"a","command":"build","cwd":"",
            "result":{"CacheHit":{"saved_duration_ms":5}}}],"exit_code":0}"#,
        )
        .unwrap();
        assert!(matches!(
            summary.tasks[0].result,
            TaskResult::CacheHit { saved_duration_ms: 5, source: CacheHitSource::Local }
        ));
    }

    #[test]
    fn upload_failure_notice_shows_a_shared_reason() {
        let summary = compact_summary(vec![
            upload_failed_task("a", saved_error("network error", &["connection refused"])),
            upload_failed_task("b", saved_error("network error", &["connection reset"])),
        ]);
        assert_eq!(
            summary.as_str(),
            "---\nvp run: 0/2 cache hit (0%), 2/2 successful. pkg#a (and 1 more) not uploaded to the remote cache: \
             network error. (Run `vp run --last-details` for full details)\n"
        );

        let summary = compact_summary(vec![
            upload_failed_task("a", saved_error("network error", &[])),
            upload_failed_task("b", saved_error("HTTP status 500", &[])),
        ]);
        assert_eq!(
            summary.as_str(),
            "---\nvp run: 0/2 cache hit (0%), 2/2 successful. pkg#a (and 1 more) not uploaded to the remote cache. \
             (Run `vp run --last-details` for full details)\n"
        );
    }

    #[test]
    fn uploads_pending_names_the_count() {
        let pending = |count| strip(&format_uploads_pending(NonZeroUsize::new(count).unwrap()));
        assert_eq!(pending(1).as_str(), "Waiting for 1 remote cache upload to finish...\n");
        assert_eq!(pending(2).as_str(), "Waiting for 2 remote cache uploads to finish...\n");
    }

    #[test]
    fn full_summary_shows_each_cause_on_its_own_line() {
        let task = upload_failed_task(
            "a",
            saved_error("network error", &["error sending request", "connection refused"]),
        );
        let summary =
            strip(&format_full_summary(&LastRunSummary { tasks: vec![task], exit_code: 0 }));
        assert!(
            summary.as_str().contains(
                "\n      ⚠ Not uploaded to the remote cache: network error\n        \
                 ↳ error sending request\n        ↳ connection refused\n"
            ),
            "{summary}"
        );
    }

    #[test]
    fn output_forwarding_error_has_distinct_message() {
        let error = ExecutionError::ForwardTaskProcessOutput(
            anyhow::anyhow!("Resource temporarily unavailable (os error 11)")
                .context("failed to read stdout"),
        );

        let saved = SavedError::new(&error);

        assert_eq!(saved.message.as_str(), "Failed to forward task process output");
        assert_eq!(
            saved.causes.iter().map(Str::as_str).collect::<Vec<_>>(),
            ["failed to read stdout", "Resource temporarily unavailable (os error 11)"]
        );
    }
}

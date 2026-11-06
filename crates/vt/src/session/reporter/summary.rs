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
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use vt_casefold::EnvName;
use vt_path::{AbsolutePath, RelativePath};
use vt_str::Str;

use super::{CACHE_MISS_STYLE, COMMAND_STYLE, ColorizeExt};
use crate::session::{
    cache::{
        BaselineDetails, BaselinePath, CacheHitSource, CacheMiss, Change, InputChangeKind,
        format_change,
    },
    event::{CacheDisabledReason, CacheNotUpdatedReason, CacheStatus, CacheUpdateStatus, RunKind},
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
pub enum TaskResult {
    /// Cache hit — output was replayed from cache. Always successful.
    CacheHit {
        saved_duration_ms: u64,
        /// Summaries saved before the source was recorded read as local hits.
        #[serde(default)]
        source: CacheHitSource,
        /// With `vp run -vv`, what the cached run depends on and its outputs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        baseline: Option<BaselineDetails>,
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

/// Why a cache miss occurred.
#[derive(Serialize, Deserialize)]
pub enum SavedCacheMissReason {
    /// No previous cache entry for this task.
    NotFound,
    /// The task's cached run differs.
    Changed {
        /// Each change, in the order they're listed.
        changes: Vec<Change>,
        /// Some files or envs couldn't be checked, so `changes` may miss some.
        #[serde(default, skip_serializing_if = "is_false")]
        changes_partial: bool,
        /// With `vp run -vv`, what the cached run depends on and its outputs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        baseline: Option<BaselineDetails>,
    },
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
            upload_failures: Vec::new(),
        };

        for task in tasks {
            match &task.result {
                TaskResult::CacheHit { saved_duration_ms, source, .. } => {
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
            CacheMiss::Changed(comparison) => Self::Changed {
                changes: comparison.changes.clone(),
                changes_partial: comparison.changes_partial,
                baseline: comparison.baseline.as_deref().cloned(),
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
    pub fn from_execution(
        cache_status: &CacheStatus,
        exit_status: Option<std::process::ExitStatus>,
        saved_error: Option<&SavedError>,
        cache_update_status: &CacheUpdateStatus,
    ) -> Self {
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
            CacheStatus::Hit { replayed_duration, source, baseline, .. } => saved_error
                .map_or_else(
                    || Self::CacheHit {
                        saved_duration_ms: duration_to_ms(*replayed_duration),
                        source: *source,
                        baseline: baseline.as_deref().cloned(),
                    },
                    |error| Self::RestoreFailed { source: *source, error: error.clone() },
                ),
            CacheStatus::Disabled(CacheDisabledReason::InProcessExecution) => Self::InProcess,
            CacheStatus::Disabled(CacheDisabledReason::NoCacheMetadata) => Self::Spawned {
                cache_status: SpawnedCacheStatus::Disabled,
                outcome: spawn_outcome_from_execution(
                    exit_status,
                    saved_error,
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
    /// lines below it: the other changes of a miss, each aligned under the
    /// first, and the causes. Only a remote read failure has causes. The
    /// caller shows [`Self::ipc_server_error`] instead, if there is one.
    ///
    /// Examples:
    /// - "→ Cache hit - output replayed - 102.96ms saved"
    /// - "→ Remote cache hit - output replayed - 102.96ms saved"
    /// - "→ Cache hit" (in a dry run, which replays nothing)
    /// - "→ Cache hit, but the outputs couldn't be restored"
    /// - "→ Cache miss: no previous cache entry found"
    /// - "→ Cache disabled in task configuration"
    fn format_cache_detail(&self, run_kind: RunKind) -> (Str, Vec<Str>, &[Str]) {
        // Tool-reported cache disable — the tool said it shouldn't be cached.
        if let Self::Spawned {
            outcome: SpawnOutcome::Success { tool_disabled_cache: true, .. },
            ..
        } = self
        {
            return (Str::from("→ Not cached: the task opted out of caching"), Vec::new(), &[]);
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
                Vec::new(),
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
                Vec::new(),
                &[],
            );
        }

        let detail = match self {
            Self::CacheHit { saved_duration_ms, source, .. } => {
                let hit = format_hit(*source);
                match run_kind {
                    RunKind::Real => {
                        let d = Duration::from_millis(*saved_duration_ms);
                        let formatted_duration = format_summary_duration(d);
                        vt_str::format!("→ {hit} - output replayed - {formatted_duration} saved")
                    }
                    RunKind::Dry => vt_str::format!("→ {hit}"),
                }
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
                    SavedCacheMissReason::Changed { changes, .. } => {
                        let mut lines = changes.iter().map(format_change);
                        let first =
                            lines.next().unwrap_or_else(|| Str::from("configuration changed"));
                        return (vt_str::format!("→ Cache miss: {first}"), lines.collect(), &[]);
                    }
                    SavedCacheMissReason::RemoteReadFailed(error) => {
                        return (
                            vt_str::format!("→ Cache miss: {}", error.message),
                            Vec::new(),
                            &error.causes,
                        );
                    }
                },
            },
        };
        (detail, Vec::new(), &[])
    }

    /// With `vp run -vv`, the cached run the task was compared with, and the
    /// changes found, or `None` for them when some files or envs couldn't be
    /// checked, so what's unchanged isn't known.
    fn baseline(&self) -> Option<(&BaselineDetails, Option<&[Change]>)> {
        match self {
            Self::CacheHit { baseline: Some(baseline), .. } => Some((baseline, Some(&[]))),
            Self::Spawned {
                cache_status:
                    SpawnedCacheStatus::Miss(SavedCacheMissReason::Changed {
                        changes,
                        changes_partial,
                        baseline: Some(baseline),
                    }),
                ..
            } => Some((baseline, (!changes_partial).then_some(changes.as_slice()))),
            _ => None,
        }
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
/// Used by both `--verbose` (live) and `--last-details` (from file). A dry
/// run's summary has no exit icons and no saved time, since nothing ran.
#[expect(
    clippy::too_many_lines,
    reason = "summary formatting is inherently verbose with many write calls"
)]
pub fn format_full_summary(summary: &LastRunSummary, run_kind: RunKind) -> Vec<u8> {
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

    if run_kind == RunKind::Real && stats.total_saved > Duration::ZERO {
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
        if run_kind == RunKind::Dry {
            // Nothing ran, so there is no exit status.
        } else if task.result.is_success() {
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
            let (cache_detail, more_changes, causes) = task.result.format_cache_detail(run_kind);
            let _ = writeln!(buf, "      {}", cache_detail.style(detail_style));
            for change in more_changes {
                // Aligned under the first change, after "→ Cache miss: ".
                let _ = writeln!(buf, "                    {}", change.style(detail_style));
            }
            write_causes(&mut buf, causes, detail_style);
            if let Some((baseline, changes)) = task.result.baseline() {
                write_baseline(&mut buf, baseline, changes);
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

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` passes a reference"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Write what `vp run -vv` shows of a cached run: what it depends on that
/// didn't change, under `Unchanged:`, and its output files, under `Outputs:`,
/// each by path, aligned under the first. Without the complete `changes`,
/// what's unchanged isn't known, and only the outputs are written.
fn write_baseline(buf: &mut Vec<u8>, baseline: &BaselineDetails, changes: Option<&[Change]>) {
    let style = Style::new().bright_black();
    let unchanged = changes.map_or_else(Vec::new, |changes| unchanged_lines(baseline, changes));
    let outputs: Vec<Str> =
        baseline.outputs.iter().flatten().map(|path| vt_str::format!("'{path}'")).collect();
    for (label, lines) in [("Unchanged:", unchanged), ("Outputs:", outputs)] {
        for (index, line) in lines.iter().enumerate() {
            let label = if index == 0 { label } else { "" };
            let _ = writeln!(buf, "        {}", vt_str::format!("{label:<11}{line}").style(style));
        }
    }
}

/// The lines of what a cached run depends on that none of `changes` changed:
/// its paths, then its env names.
fn unchanged_lines(baseline: &BaselineDetails, changes: &[Change]) -> Vec<Str> {
    let mut changed_paths: FxHashSet<&RelativePath> = FxHashSet::default();
    // The folders whose listing changed: an entry was added or removed, not
    // just changed.
    let mut changed_listings: FxHashSet<&RelativePath> = FxHashSet::default();
    // Env names compare by the platform's rules, as the changes do.
    let mut changed_envs: FxHashSet<EnvName<&str>> = FxHashSet::default();
    for change in changes {
        match change {
            Change::File { path, kind } => {
                changed_paths.insert(path.as_relative_path());
                if matches!(kind, InputChangeKind::Added | InputChangeKind::Removed)
                    && let Some(parent) = path.as_relative_path().parent()
                {
                    changed_listings.insert(parent);
                }
            }
            Change::Env { mismatch, .. } => {
                changed_envs.insert(EnvName::new(mismatch.name().as_str()));
            }
            _ => {}
        }
    }
    let mut unchanged: Vec<Str> = baseline
        .paths
        .iter()
        .filter(|(path, recorded)| {
            let path = path.as_relative_path();
            let listed = matches!(recorded, BaselinePath::Dir { entries: Some(_), .. });
            let differs =
                changed_paths.contains(path) || (listed && changed_listings.contains(path));
            !differs
        })
        .map(|(path, recorded)| {
            let path = quoted_path(path);
            match recorded {
                BaselinePath::File { .. } => path,
                BaselinePath::Dir { entries: Some(_), .. } => vt_str::format!("{path} (listing)"),
                BaselinePath::Dir { entries: None, .. } => vt_str::format!("{path} (folder)"),
                BaselinePath::Missing => vt_str::format!("{path} (missing)"),
            }
        })
        .collect();
    unchanged.extend(
        baseline
            .envs
            .iter()
            .filter(|name| !changed_envs.contains(&EnvName::new(name.as_str())))
            .map(|name| vt_str::format!("env '{name}'")),
    );
    unchanged
}

/// `path` in quotes, or `workspace root` for the root itself, as the miss
/// reasons name it.
fn quoted_path(path: &RelativePath) -> Str {
    if path.as_str().is_empty() {
        Str::from("workspace root")
    } else {
        vt_str::format!("'{}'", path.as_str())
    }
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
///
/// A dry run's summary follows the same rules, with "nothing was run" in place of the
/// successful count and saved time. Its tasks print no output, so a blank line comes before the
/// thin line, and there is no `--last-details` hint: that shows the last real run.
pub fn format_compact_summary(
    summary: &LastRunSummary,
    program_name: &str,
    run_kind: RunKind,
) -> Vec<u8> {
    if summary.tasks.is_empty() {
        return Vec::new();
    }

    let stats = SummaryStats::compute(&summary.tasks);

    let is_single_task = summary.tasks.len() == 1;

    // Single task + not cache hit + no notice → no summary
    if is_single_task && stats.cache_hits == 0 && stats.upload_failures.is_empty() {
        return Vec::new();
    }

    let mut buf = Vec::new();

    if run_kind == RunKind::Dry {
        let _ = writeln!(buf);
    }

    // Thin line separator
    let _ = writeln!(buf, "{}", "---".style(Style::new().bright_black()));

    let run_label = vt_str::format!("{program_name} run:");
    let mut show_last_details_hint = run_kind == RunKind::Real;
    if is_single_task && stats.cache_hits > 0 {
        // Single task cache hit — no need for --last-details hint
        let hit = if stats.remote_cache_hits > 0 { "remote cache hit" } else { "cache hit" };
        let _ = write!(buf, "{} {hit}, ", run_label.as_str().style(Style::new().blue().bold()));
        match run_kind {
            RunKind::Real => {
                let formatted_total_saved = format_summary_duration(stats.total_saved);
                let _ = write!(
                    buf,
                    "{} saved.",
                    formatted_total_saved.style(Style::new().green().bold())
                );
            }
            RunKind::Dry => {
                let _ = write!(buf, "nothing was run.");
            }
        }
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

        match run_kind {
            RunKind::Real => {
                let total = stats.total;
                let successful = total - stats.failed;
                let successful_style =
                    if successful < total { Style::new().red() } else { Style::new() };
                let _ = write!(
                    buf,
                    " {} successful",
                    vt_str::format!("{successful}/{total}").style(successful_style)
                );

                if stats.total_saved > Duration::ZERO {
                    let formatted_total_saved = format_summary_duration(stats.total_saved);
                    let _ = write!(
                        buf,
                        ", {} saved",
                        formatted_total_saved.style(Style::new().green().bold())
                    );
                }
            }
            RunKind::Dry => {
                let _ = write!(buf, " nothing was run");
            }
        }

        let _ = write!(buf, ".");
    } else {
        // Single task, no cache hit — only shown with a notice below
        let _ = write!(buf, "{}", run_label.as_str().style(Style::new().blue().bold()));
    }

    // Inline notices before the --last-details hint
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
    use std::collections::BTreeMap;

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
            result: TaskResult::CacheHit { saved_duration_ms: 1000, source, baseline: None },
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

    fn compact_summary(tasks: Vec<TaskSummary>) -> Str {
        strip(&format_compact_summary(&LastRunSummary { tasks, exit_code: 0 }, "vp", RunKind::Real))
    }

    fn full_summary(tasks: Vec<TaskSummary>) -> Str {
        strip(&format_full_summary(&LastRunSummary { tasks, exit_code: 0 }, RunKind::Real))
    }

    #[test]
    fn full_summary_lists_each_change_on_its_own_line() {
        let mut task = cache_miss_task("a");
        if let TaskResult::Spawned { cache_status, .. } = &mut task.result {
            *cache_status = SpawnedCacheStatus::Miss(SavedCacheMissReason::Changed {
                changes: vec![Change::InputConfig, Change::OutputConfig],
                changes_partial: false,
                baseline: None,
            });
        }
        let summary = full_summary(vec![task]);
        let lines: Vec<&str> = summary.as_str().lines().collect();
        assert!(
            lines.windows(2).any(|pair| pair
                == [
                    "      → Cache miss: input configuration changed",
                    "                    output configuration changed",
                ]),
            "{summary}"
        );
    }

    fn baseline_details() -> BaselineDetails {
        let rel = |path| RelativePathBuf::new(path).unwrap();
        BaselineDetails {
            envs: vec![Str::from("NODE_ENV")],
            paths: BTreeMap::from([
                (
                    rel("src"),
                    BaselinePath::Dir {
                        entries: Some(vec![Str::from("a.txt")]),
                        may_be_missing: false,
                    },
                ),
                (rel("src/a.txt"), BaselinePath::File { hash: 1, may_be_missing: false }),
            ]),
            outputs: Some(vec![Str::from("dist/out.txt")]),
        }
    }

    fn baseline_text(changes: Option<&[Change]>) -> Str {
        let mut buf = Vec::new();
        write_baseline(&mut buf, &baseline_details(), changes);
        strip(&buf)
    }

    /// A changed file's content leaves the folder listing it unchanged, and
    /// an added entry changes it.
    #[test]
    fn unchanged_leaves_out_only_what_changed() {
        let file = |kind, path| Change::File { kind, path: RelativePathBuf::new(path).unwrap() };
        assert_eq!(
            baseline_text(Some(&[file(InputChangeKind::ContentModified, "src/a.txt")])).as_str(),
            "        Unchanged: 'src' (listing)\n                   env 'NODE_ENV'\n        \
             Outputs:   'dist/out.txt'\n"
        );
        assert_eq!(
            baseline_text(Some(&[file(InputChangeKind::Added, "src/b.txt")])).as_str(),
            "        Unchanged: 'src/a.txt'\n                   env 'NODE_ENV'\n        \
             Outputs:   'dist/out.txt'\n"
        );
    }

    /// The workspace root reads as in the miss reasons, not as `''`.
    #[test]
    fn workspace_root_is_named() {
        let root = RelativePathBuf::new("").unwrap();
        let baseline = BaselineDetails {
            envs: Vec::new(),
            paths: BTreeMap::from([(
                root,
                BaselinePath::Dir {
                    entries: Some(vec![Str::from("a.txt")]),
                    may_be_missing: false,
                },
            )]),
            outputs: Some(Vec::new()),
        };
        let mut buf = Vec::new();
        write_baseline(&mut buf, &baseline, Some(&[]));
        assert_eq!(strip(&buf).as_str(), "        Unchanged: workspace root (listing)\n");
    }

    /// A changed env isn't unchanged under another spelling that the
    /// platform treats as the same name: on Windows, `node_env` is `NODE_ENV`.
    #[test]
    fn changed_envs_compare_by_platform_rules() {
        let change = Change::Env {
            mismatch: crate::session::cache::EnvMismatch::Changed { name: Str::from("node_env") },
            source: crate::session::cache::EnvSource::Tracked,
        };
        let text = baseline_text(Some(&[change]));
        assert_eq!(text.as_str().contains("env 'NODE_ENV'"), !cfg!(windows), "{text}");
    }

    /// When some files or envs couldn't be checked, what's unchanged isn't
    /// known, so only the outputs are listed.
    #[test]
    fn partial_changes_list_only_the_outputs() {
        assert_eq!(baseline_text(None).as_str(), "        Outputs:   'dist/out.txt'\n");
    }

    /// A dry run whose tasks all have caching disabled still gets its
    /// summary line. The other dry-run summaries are in the `dry_run` e2e
    /// snapshots.
    #[test]
    fn dry_run_without_caching_says_nothing_was_run() {
        let tasks = vec![cache_disabled_task("a"), cache_disabled_task("b")];
        let summary =
            format_compact_summary(&LastRunSummary { tasks, exit_code: 0 }, "vp", RunKind::Dry);
        assert_eq!(strip(&summary).as_str(), "\n---\nvp run: nothing was run.\n");
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
            TaskResult::CacheHit { saved_duration_ms: 5, source: CacheHitSource::Local, .. }
        ));
    }

    /// A miss saved before `-vv` existed, without `changes_partial` or
    /// `baseline`, still reads.
    #[test]
    fn saved_miss_without_baseline_reads() {
        let reason: SavedCacheMissReason =
            serde_json::from_str(r#"{"Changed":{"changes":["InputConfig"]}}"#).unwrap();
        assert!(matches!(
            reason,
            SavedCacheMissReason::Changed { ref changes, changes_partial: false, baseline: None }
                if changes.as_slice() == [Change::InputConfig]
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
        let summary = strip(&format_full_summary(
            &LastRunSummary { tasks: vec![task], exit_code: 0 },
            RunKind::Real,
        ));
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

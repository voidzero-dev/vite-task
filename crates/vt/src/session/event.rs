use std::{
    num::NonZeroI32,
    process::ExitStatus,
    sync::{Arc, OnceLock},
    time::Duration,
};

use vt_server::Error as IpcServerError;
use vt_str::Str;

use super::cache::{BaselineDetails, CacheHitSource, CacheMiss, remote::UploadError};

/// The cache operation that failed.
#[derive(Debug)]
pub enum CacheErrorKind {
    /// Cache lookup (`try_hit`) failed.
    Lookup,
    /// Restoring the output files of a remote cache hit failed. A local hit
    /// fails with [`ExecutionError::LocalCacheRestore`] instead.
    Restore,
    /// Writing the cache entry failed after successful execution.
    Update,
}

impl std::fmt::Display for CacheErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lookup => f.write_str("lookup"),
            Self::Restore => f.write_str("restore"),
            Self::Update => f.write_str("update"),
        }
    }
}

/// Error that occurred during a leaf execution.
///
/// Reported through [`super::reporter::LeafExecutionReporter::finish()`] and
/// displayed by the reporter.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    /// A cache operation failed.
    #[error("Cache {kind} failed")]
    Cache {
        kind: CacheErrorKind,
        #[source]
        source: anyhow::Error,
    },

    /// Restoring the output files of a local cache hit failed. The entry
    /// stays in the cache, so later runs fail the same way until the cache is
    /// cleared.
    #[error("Cache restore failed. Run `{program_name} cache clean` to clear the cache")]
    LocalCacheRestore {
        program_name: Str,
        #[source]
        source: anyhow::Error,
    },

    /// The OS failed to spawn the child process (e.g., command not found).
    #[error("Failed to spawn process")]
    Spawn(#[source] anyhow::Error),

    /// The child process started, but the runner failed while forwarding its output.
    #[error("Failed to forward task process output")]
    ForwardTaskProcessOutput(#[source] anyhow::Error),

    /// The child process started, but the runner failed while waiting for it to exit.
    #[error("Failed to wait for task process to exit")]
    WaitForTaskProcessExit(#[source] anyhow::Error),

    /// Creating the post-run fingerprint failed after successful execution.
    #[error("Failed to create post-run fingerprint")]
    PostRunFingerprint(#[source] anyhow::Error),

    /// The runner-aware IPC server failed to bind for this task. Reported
    /// instead of silently degrading so that `{ auto: true }` inputs stay
    /// observable end-to-end.
    #[error("Failed to set up task communication")]
    IpcServerBind(#[source] std::io::Error),
}

#[derive(Debug, Clone)]
pub enum CacheDisabledReason {
    InProcessExecution,
    NoCacheMetadata,
}

/// Why the cache was not updated after a spawned process exited with status 0.
#[derive(Debug)]
pub enum CacheNotUpdatedReason {
    /// Caching was disabled for this task.
    CacheDisabled,
    /// Execution was cancelled before the result could be trusted.
    /// Two possible causes:
    /// - Ctrl-C: the user interrupted execution; the task may have
    ///   exited successfully but without completing its intended work.
    /// - Fast-fail: a sibling task failed, triggering cancellation
    ///   while this task was still running.
    Cancelled,
    /// fspy isn't compiled in on this build and the task requires fspy
    /// (its `input` or `output` config includes auto-inference). Task ran but
    /// cannot be cached without tracked path accesses.
    FspyUnsupported,
    /// The runner's IPC server failed during execution, so the collected
    /// reports may be incomplete. Caching such a run would risk stale
    /// inputs/outputs on the next hit. Carries the underlying error for
    /// user-facing reporting.
    IpcServerError(IpcServerError),
    /// A runner-aware tool explicitly requested that this run not be cached
    /// (e.g. vite dev-server, a watch task).
    ToolRequested,
    /// A tracked process could not record everything it did. For example,
    /// the channel it reports file accesses through had no room left, or
    /// the seccomp filter could not be installed and an exec continued
    /// untracked. The accesses that did arrive are a subset of what the
    /// task touched, so caching from them would bake in inputs and outputs
    /// that are not all of them.
    #[cfg_attr(not(fspy), expect(dead_code, reason = "only fspy tracking can be incomplete"))]
    TrackingIncomplete,
}

/// Outcome of the post-run cache update after a spawned process exited with
/// status 0.
#[derive(Debug)]
pub enum CacheUpdateOutcome {
    /// Cache was successfully updated with new fingerprint and outputs.
    Updated {
        /// Why uploading the entry to the remote cache failed. It's set only
        /// if the upload fails, which can happen in the background after the
        /// task finishes. Empty if the upload succeeded or wasn't attempted.
        /// Read it only after
        /// [`ExecutionCache::wait_for_uploads`](super::cache::ExecutionCache::wait_for_uploads)
        /// returns.
        upload_error: Arc<OnceLock<UploadError>>,
    },
    /// Cache was not updated for a non-error reason.
    NotUpdated(CacheNotUpdatedReason),
    /// Updating the cache failed with an infrastructure error (post-run
    /// fingerprint creation, output archiving, or writing the cache entry).
    Failed(ExecutionError),
}

/// Final outcome of a single leaf execution, passed to
/// [`LeafExecutionReporter::finish`](super::reporter::LeafExecutionReporter::finish).
///
/// Each variant carries only the data valid for that outcome, so invalid
/// combinations (such as an execution error paired with a cache update, or a
/// cache update on a non-zero exit) are unrepresentable.
#[derive(Debug)]
pub enum LeafOutcome {
    /// An infrastructure error prevented the process from running, prevented
    /// observing it to completion, or prevented restoring a cache hit's outputs.
    Failed(ExecutionError),
    /// Cache hit: no process ran. Captured outputs were replayed, except in a
    /// dry run.
    CacheHit,
    /// In a dry run, a miss or a task without caching: nothing ran.
    WouldSpawn,
    /// An in-process (built-in) command completed (or was skipped in a dry run).
    InProcess,
    /// The spawned process exited with status 0.
    SpawnedSuccess { cache_update: CacheUpdateOutcome },
    /// The spawned process exited with a non-zero status; the cache is never
    /// updated.
    SpawnedFailed { exit_code: NonZeroI32 },
    /// The run was cancelled before the process started.
    Cancelled,
}

/// Whether a run runs its commands, or is a dry run (`vp run --dry-run`),
/// which plans them and checks the cache without running anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Real,
    Dry,
}

#[derive(Debug, Clone)]
pub enum CacheStatus {
    Disabled(CacheDisabledReason),
    Miss(CacheMiss),
    Hit {
        replayed_duration: Duration,
        source: CacheHitSource,
        /// The hit has output files, which a real run restores.
        restores_outputs: bool,
        /// With `vp run -vv`, what the cached run depends on and its outputs.
        baseline: Option<Arc<BaselineDetails>>,
    },
}

/// Convert a non-success `ExitStatus` to a non-zero exit code.
/// On Unix, if terminated by signal, returns `128 + signal_number`.
/// Falls back to `1` if the OS reports neither a non-zero code nor a signal.
pub fn non_zero_exit_code(status: ExitStatus) -> NonZeroI32 {
    #[cfg(unix)]
    let code = {
        use std::os::unix::process::ExitStatusExt;
        status.code().unwrap_or_else(|| {
            // Process was terminated by signal, use Unix convention: 128 + signal
            status.signal().map_or(1, |sig| 128 + sig)
        })
    };
    #[cfg(not(unix))]
    let code = {
        // Windows always has an exit code
        status.code().unwrap_or(1)
    };
    NonZeroI32::new(code).unwrap_or(const { NonZeroI32::new(1).unwrap() })
}

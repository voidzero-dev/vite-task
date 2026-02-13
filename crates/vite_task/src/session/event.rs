use std::{process::ExitStatus, time::Duration};

use super::cache::CacheMiss;

#[derive(Debug)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub enum CacheDisabledReason {
    InProcessExecution,
    NoCacheMetadata,
}

#[derive(Debug)]
pub enum CacheNotUpdatedReason {
    /// Cache was hit - task was replayed from cache, no update needed
    CacheHit,
    /// Caching was disabled for this task
    CacheDisabled,
    /// Execution exited with non-zero status
    NonZeroExitStatus,
}

#[derive(Debug)]
pub enum CacheUpdateStatus {
    /// Cache was successfully updated with new fingerprint and outputs
    Updated,
    /// Cache was not updated (with reason).
    /// The reason is part of the `LeafExecutionReporter` trait contract — reporters
    /// can use it for detailed logging, even if current implementations don't.
    NotUpdated(
        #[expect(
            dead_code,
            reason = "part of LeafExecutionReporter trait contract; reporters may use for detailed logging"
        )]
        CacheNotUpdatedReason,
    ),
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "CacheMiss variant is intentionally large and infrequently cloned"
)]
pub enum CacheStatus {
    Disabled(CacheDisabledReason),
    Miss(CacheMiss),
    Hit { replayed_duration: Duration },
}

/// Convert `ExitStatus` to an i32 exit code.
/// On Unix, if terminated by signal, returns 128 + `signal_number`.
pub fn exit_status_to_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.code().unwrap_or_else(|| {
            // Process was terminated by signal, use Unix convention: 128 + signal
            status.signal().map_or(1, |sig| 128 + sig)
        })
    }
    #[cfg(not(unix))]
    {
        // Windows always has an exit code
        status.code().unwrap_or(1)
    }
}

use std::{process::ExitStatus, time::Duration};

use bstr::BString;
// Re-export ExecutionItemDisplay from vite_task_plan since it's the canonical definition
pub use vite_task_plan::ExecutionItemDisplay;

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
    CycleDetected,
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
    /// Cache was not updated (with reason)
    NotUpdated(CacheNotUpdatedReason),
}

#[derive(Debug)]
pub enum CacheStatus {
    Disabled(CacheDisabledReason),
    Miss(CacheMiss),
    Hit { replayed_duration: Duration },
}

/// Convert ExitStatus to an i32 exit code.
/// On Unix, if terminated by signal, returns 128 + signal_number.
pub fn exit_status_to_code(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.code().unwrap_or_else(|| {
            // Process was terminated by signal, use Unix convention: 128 + signal
            status.signal().map(|sig| 128 + sig).unwrap_or(1)
        })
    }
    #[cfg(not(unix))]
    {
        // Windows always has an exit code
        status.code().unwrap_or(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutionId(u32);

impl ExecutionId {
    pub(crate) fn zero() -> Self {
        Self(0)
    }

    pub(crate) fn next(&self) -> Self {
        Self(self.0.checked_add(1).expect("ExecutionId overflow"))
    }
}

#[derive(Debug)]
pub struct ExecutionEvent {
    pub execution_id: ExecutionId,
    pub kind: ExecutionEventKind,
}

#[derive(Debug)]
pub enum ExecutionEventKind {
    Start { display: Option<ExecutionItemDisplay>, cache_status: CacheStatus },
    Output { kind: OutputKind, content: BString },
    Error { message: String },
    Finish { status: Option<ExitStatus>, cache_update_status: CacheUpdateStatus },
}

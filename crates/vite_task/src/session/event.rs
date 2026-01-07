use std::time::Duration;

use bstr::BString;
// Re-export ExecutionItemDisplay from vite_task_plan since it's the canonical definition
pub use vite_task_plan::ExecutionItemDisplay;

#[derive(Debug)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

#[derive(Debug)]
pub enum CacheDisabledReason {
    InProcessExecution,
}

#[derive(Debug)]
pub enum CacheStatus {
    Disabled(CacheDisabledReason),
    Miss,
    Hit { replayed_duration: Duration },
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

pub struct ExecutionStartedEvent {
    pub execution_id: ExecutionId,
    pub display: ExecutionItemDisplay,
}

pub struct ExecutionOutputEvent {
    pub execution_id: ExecutionId,
    pub kind: OutputKind,
    pub content: BString,
}

#[derive(Debug)]
pub struct ExecutionEvent {
    pub execution_id: ExecutionId,
    pub kind: ExecutionEventKind,
}

#[derive(Debug)]
pub enum ExecutionEventKind {
    Start(Option<ExecutionItemDisplay>),
    Output { kind: OutputKind, content: BString },
    Error { message: String },
    Finish { status: Option<i32>, cache_status: CacheStatus },
}

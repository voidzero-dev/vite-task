use std::sync::Arc;

use bstr::BString;
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::collections::HashMap;

#[derive(Clone, Debug)]
pub struct ExecutionStartInfo {
    /// None if the execution is not associated with a specific task, but directly synthesized from CLI args, like `vite lint`/`vite exec ...`
    pub task_display_name: Option<Str>,
    pub command: Str,
    pub cwd: Arc<AbsolutePath>,
}

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
    pub display: ExecutionStartInfo,
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
    Start(ExecutionStartInfo),
    Output { kind: OutputKind, content: BString },
    Finish { status: Option<i32>, cache_status: CacheStatus },
}

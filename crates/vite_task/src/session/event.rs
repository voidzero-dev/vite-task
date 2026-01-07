use std::sync::Arc;

use bstr::BString;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::display::TaskDisplay;

#[derive(Clone, Debug)]
pub struct ExecutionItemDisplay {
    pub task_display: TaskDisplay,
    pub and_item_index: Option<usize>,
    pub command: Str,
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
    Start(ExecutionItemDisplay),
    Output { kind: OutputKind, content: BString },
    Finish { status: Option<i32>, cache_status: CacheStatus },
}

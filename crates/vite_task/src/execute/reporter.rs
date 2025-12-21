use std::sync::Arc;

use vite_path::AbsolutePath;
use vite_str::Str;

use crate::collections::HashMap;

pub struct ExecutionDisplay {
    command: Str,
    cwd: Arc<AbsolutePath>,
}

pub enum OutputKind {
    Stdout,
    Stderr,
}

pub enum ExecutionEvent {
    Output { kind: OutputKind, content: Vec<u8> },
    Finished { status: Option<i32>, cache_status: () },
}

pub trait Reporter {
    fn new_execution(&mut self, display: ExecutionDisplay);
}

//! What a task can rebuild, so a later run may find it missing.
//!
//! Inputs are fingerprinted as the task left them, so they include the files
//! a task read and then wrote, such as a tool's cache file. A clean checkout
//! (CI, a fresh clone) doesn't have those files yet, and running the task
//! would write them again, so a later run may find them missing. A file that
//! is there must still be as the task left it.
//!
//! The written paths are fspy's records before output exclusions and
//! tool-reported `ignoreOutput` paths are applied: those decide what is
//! archived, not what the task can rebuild.

use rustc_hash::FxHashSet;
use vt_path::{RelativePath, RelativePathBuf};

/// The paths a task wrote, relative to the workspace root.
#[derive(Debug, Default)]
pub struct Rebuildable {
    written: FxHashSet<RelativePathBuf>,
}

impl Rebuildable {
    pub const fn new(written: FxHashSet<RelativePathBuf>) -> Self {
        Self { written }
    }

    /// Whether a later run may find `path` missing: the task wrote it.
    pub fn may_be_missing(&self, path: &RelativePath) -> bool {
        self.written.contains(path)
    }
}

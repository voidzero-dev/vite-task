//! What a task can rebuild, so a later run may find it missing.
//!
//! Inputs are fingerprinted as the task left them, so they include the files
//! a task read and then wrote, such as a tool's cache file. A clean checkout
//! (CI, a fresh clone) doesn't have those files yet, and running the task
//! would write them again, so a later run may find them missing. A file that
//! is there must still be as the task left it.
//!
//! The same goes for a folder that holds only what the task can rebuild:
//! every file (or symlink) in it is one the task wrote, and every folder in
//! it holds only what the task can rebuild too. A hit restores the files,
//! which recreates the folder, so it isn't fingerprinted at all. Such folders
//! are found after the run by climbing from the written paths through their
//! parent folders, listing each one in the real file system. A written path
//! or folder that is gone by then plays no part. A listing of any other
//! folder leaves out its entries that the task can rebuild, which a later run
//! may find missing too.
//!
//! The written paths are fspy's records before output exclusions and
//! tool-reported `ignoreOutput` paths are applied: those decide what is
//! archived, not what the task can rebuild.

use std::{cmp::Reverse, fs, io};

use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

/// The paths a task wrote and the folders holding only what it can rebuild,
/// relative to the workspace root.
#[derive(Debug, Default)]
pub struct Rebuildable {
    written: FxHashSet<RelativePathBuf>,
    folders: FxHashSet<RelativePathBuf>,
}

impl Rebuildable {
    /// Finds the rebuildable folders above the `written` paths by listing them
    /// under `workspace_root`, which must be done after the run.
    pub fn new(written: FxHashSet<RelativePathBuf>, workspace_root: &AbsolutePath) -> Self {
        let folders = find_folders(&written, workspace_root);
        Self { written, folders }
    }

    /// Whether a later run may find `path` missing: the task wrote it.
    pub fn may_be_missing(&self, path: &RelativePath) -> bool {
        self.written.contains(path)
    }

    /// Whether `path` is a folder holding only what the task can rebuild.
    pub fn is_rebuildable_folder(&self, path: &RelativePath) -> bool {
        self.folders.contains(path)
    }

    /// Whether the entry `name` of the folder `dir`, itself a folder if
    /// `is_dir`, is something the task can rebuild, so a listing of `dir` may
    /// lack it.
    pub fn is_rebuildable_entry(&self, dir: &RelativePath, name: &str, is_dir: bool) -> bool {
        RelativePathBuf::new(name)
            .is_ok_and(|name| is_rebuildable(&dir.join(name), is_dir, &self.written, &self.folders))
    }
}

/// Finds the folders holding only what the task can rebuild. Each folder above
/// a written path is listed once, deepest first, so a folder's subfolders are
/// decided before it. Climbing stops at the first folder that isn't
/// rebuildable, since no folder above it can be, and at the workspace root. A
/// folder that is gone plays no part: its parent is decided by the entries it
/// holds.
fn find_folders(
    written: &FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    let mut candidates = FxHashSet::default();
    for path in written {
        let mut dir = path.parent();
        // A folder already reached from another path has its parents too.
        while let Some(parent) = dir
            && !parent.as_str().is_empty()
            && candidates.insert(parent)
        {
            dir = parent.parent();
        }
    }
    let mut candidates: Vec<&RelativePath> = candidates.into_iter().collect();
    candidates.sort_unstable_by_key(|dir| Reverse(dir.as_str().matches('/').count()));

    let mut folders = FxHashSet::default();
    let mut above_unrebuildable = FxHashSet::default();
    for dir in candidates {
        let rebuildable = if above_unrebuildable.contains(dir) {
            Some(false)
        } else {
            holds_only_rebuildable(dir, workspace_root, written, &folders)
        };
        match rebuildable {
            Some(true) => {
                folders.insert(dir.to_relative_path_buf());
            }
            Some(false) => {
                if let Some(parent) = dir.parent() {
                    above_unrebuildable.insert(parent);
                }
            }
            None => {}
        }
    }
    folders
}

/// Whether `dir` holds at least one entry, and every entry is a file (or
/// symlink) in `written` or a folder in `folders`, or `None` when `dir` isn't
/// a folder anymore. `.DS_Store` doesn't count, as in directory listing
/// fingerprints. An empty folder isn't rebuildable: there's no written file to
/// recreate it.
fn holds_only_rebuildable(
    dir: &RelativePath,
    workspace_root: &AbsolutePath,
    written: &FxHashSet<RelativePathBuf>,
    folders: &FxHashSet<RelativePathBuf>,
) -> Option<bool> {
    let entries = match fs::read_dir(workspace_root.join(dir)) {
        Ok(entries) => entries,
        // Gone, or not a folder anymore: it plays no part.
        Err(err)
            if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) =>
        {
            return None;
        }
        Err(_) => return Some(false),
    };
    let mut is_empty = true;
    for entry in entries {
        let Ok(entry) = entry else {
            return Some(false);
        };
        let name = entry.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let (Some(name), Ok(file_type)) = (name.to_str(), entry.file_type()) else {
            return Some(false);
        };
        let Ok(name) = RelativePathBuf::new(name) else {
            return Some(false);
        };
        if !is_rebuildable(&dir.join(name), file_type.is_dir(), written, folders) {
            return Some(false);
        }
        is_empty = false;
    }
    Some(!is_empty)
}

/// Whether the entry at `path` is rebuildable: a folder in `folders`, or a
/// file (or symlink) in `written`.
fn is_rebuildable(
    path: &RelativePath,
    is_dir: bool,
    written: &FxHashSet<RelativePathBuf>,
    folders: &FxHashSet<RelativePathBuf>,
) -> bool {
    if is_dir { folders.contains(path) } else { written.contains(path) }
}

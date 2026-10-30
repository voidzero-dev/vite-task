//! What a task can rebuild, so a later run may find it missing.
//!
//! Inputs are fingerprinted as the task left them, so they include the files
//! a task read and then wrote, such as a tool's cache file. A clean checkout
//! (CI, a fresh clone) doesn't have those files yet, and running the task
//! would write them again, so a later run may find them missing. A file that
//! is there must still be as the task left it.
//!
//! The same goes for an empty folder the task made (`.DS_Store` aside), and
//! for a folder that holds only what the task can rebuild: every file (or
//! symlink) in it is one the task wrote, and every folder in it is rebuildable
//! too. A hit restores the files, which recreates the folders holding them,
//! and the empty folders, which are archived as folders, so none of these is
//! fingerprinted at all. Such folders are found after the run by climbing
//! from the written paths and the empty folders through their parent folders,
//! listing each one in the real file system. A written path or folder that
//! is gone by then plays no part. A listing of any other folder leaves out
//! its entries that the task can rebuild, which a later run may find missing
//! too.
//!
//! The written paths are fspy's records before output exclusions and
//! tool-reported `ignoreOutput` paths are applied: those decide what is
//! archived, not what the task can rebuild. A rename target that is a folder
//! after the run, like Vite's `deps` folder renamed into place, stands for
//! writes of every file (and symlink) under it; the folder itself and the
//! folders in it don't count as written. The files are archived too. A write
//! record on a path that is a folder, from opening it with write access, is
//! ignored. A mkdir record counts only for a folder that is empty after the
//! run, whether or not the call made it, since it may have been there
//! already: what any other folder holds decides whether it is rebuildable.

use std::{cmp::Reverse, fs, io};

use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

/// The paths a task wrote, the empty folders it made, and the folders
/// holding only what it can rebuild, relative to the workspace root.
#[derive(Debug, Default)]
pub struct Rebuildable {
    written: FxHashSet<RelativePathBuf>,
    empty_folders: FxHashSet<RelativePathBuf>,
    /// The rebuildable folders, `empty_folders` among them.
    folders: FxHashSet<RelativePathBuf>,
}

impl Rebuildable {
    /// Takes the `written` paths and the `rename_targets` as written, with the
    /// files under a renamed folder in place of the folder and without written
    /// paths that are folders, and the `mkdirs` that are empty folders, and
    /// finds the rebuildable folders above them, by looking under
    /// `workspace_root`, which must be done after the run.
    pub fn new(
        written: FxHashSet<RelativePathBuf>,
        rename_targets: FxHashSet<RelativePathBuf>,
        mkdirs: FxHashSet<RelativePathBuf>,
        workspace_root: &AbsolutePath,
    ) -> Self {
        let written = files_written(written, rename_targets, workspace_root);
        let empty_folders: FxHashSet<RelativePathBuf> =
            mkdirs.into_iter().filter(|dir| is_empty_folder(dir, workspace_root)).collect();
        let folders = find_folders(&written, &empty_folders, workspace_root);
        Self { written, empty_folders, folders }
    }

    /// The paths the task wrote or renamed a file to, and the files under the
    /// folders it renamed.
    pub const fn written(&self) -> &FxHashSet<RelativePathBuf> {
        &self.written
    }

    /// The empty folders the task made. No written file recreates them, so
    /// they're archived as folders.
    pub const fn empty_folders(&self) -> &FxHashSet<RelativePathBuf> {
        &self.empty_folders
    }

    /// Whether a later run may find `path` missing: the task wrote it.
    pub fn may_be_missing(&self, path: &RelativePath) -> bool {
        self.written.contains(path)
    }

    /// Whether `path` is an empty folder the task made, or a folder holding
    /// only what the task can rebuild.
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

/// The paths in `written`, the `rename_targets` that aren't folders under
/// `workspace_root`, and every file (or symlink) under the rename targets that
/// are, recursively. A symlink to a folder is a file here, as in
/// [`holds_only_rebuildable`]. Entries that can't be read or named are left
/// out. A written path that is a folder, from opening it with write access,
/// stays without a stat: nothing under it counts, folder entries are checked
/// against the rebuildable folders, and the archive skips folders, so only a
/// read of it has to tell (see [`PostRunFingerprint::create`]).
///
/// [`PostRunFingerprint::create`]: super::fingerprint::PostRunFingerprint::create
fn files_written(
    mut written: FxHashSet<RelativePathBuf>,
    rename_targets: FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    for path in rename_targets {
        if fs::symlink_metadata(workspace_root.join(&path)).is_ok_and(|meta| meta.is_dir()) {
            add_files_under(&path, workspace_root, &mut written);
        } else {
            written.insert(path);
        }
    }
    written
}

/// Adds every file (or symlink) under the folder `dir` to `files`. `.DS_Store`
/// doesn't count, as in directory listing fingerprints.
fn add_files_under(
    dir: &RelativePath,
    workspace_root: &AbsolutePath,
    files: &mut FxHashSet<RelativePathBuf>,
) {
    let Ok(entries) = fs::read_dir(workspace_root.join(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let (Some(name), Ok(file_type)) = (name.to_str(), entry.file_type()) else {
            continue;
        };
        let Ok(name) = RelativePathBuf::new(name) else {
            continue;
        };
        let path = dir.join(name);
        if file_type.is_dir() {
            add_files_under(&path, workspace_root, files);
        } else {
            files.insert(path);
        }
    }
}

/// Whether `dir` is a folder under `workspace_root` holding nothing but
/// `.DS_Store`, which doesn't count, as in directory listing fingerprints.
fn is_empty_folder(dir: &RelativePath, workspace_root: &AbsolutePath) -> bool {
    let path = workspace_root.join(dir);
    fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir())
        && fs::read_dir(path).is_ok_and(|mut entries| {
            entries.all(|entry| entry.is_ok_and(|entry| entry.file_name() == ".DS_Store"))
        })
}

/// Finds the folders holding only what the task can rebuild, starting with
/// the `empty_folders`. Each folder above a written path or an empty folder is
/// listed once, deepest first, so a folder's subfolders are decided before
/// it. Climbing stops at the first folder that isn't rebuildable, since no
/// folder above it can be, and at the workspace root. A folder that is gone
/// plays no part: its parent is decided by the entries it holds.
fn find_folders(
    written: &FxHashSet<RelativePathBuf>,
    empty_folders: &FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> FxHashSet<RelativePathBuf> {
    let mut candidates = FxHashSet::default();
    for path in written.iter().chain(empty_folders) {
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

    let mut folders = empty_folders.clone();
    let mut above_unrebuildable = FxHashSet::default();
    for dir in candidates {
        // An empty folder holds no written path, so it is only a candidate
        // above written paths that are gone.
        if folders.contains(dir) {
            continue;
        }
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
/// fingerprints. An empty folder that isn't in `folders` isn't rebuildable:
/// the task didn't make it, so a hit wouldn't recreate it.
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

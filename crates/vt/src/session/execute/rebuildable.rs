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
//! too. None of these is fingerprinted at all. A hit restores the files and
//! the empty folders, which are archived as folders, if they're among the
//! task's outputs, and the files recreate the folders holding them; explicit
//! output config can leave them out. Such folders are found after the run by
//! climbing from the written paths and the empty folders through their parent
//! folders inside the workspace, listing each one once in the real file
//! system. A written path or folder that is gone by then plays no part. A
//! listing of any other folder leaves out its entries that the task can
//! rebuild, which a later run may find missing too.
//!
//! The written paths are fspy's records before output exclusions and
//! tool-reported `ignoreOutput` paths are applied: those decide what is
//! archived, not what the task can rebuild. A rename target that is a folder
//! after the run, like Vite's `deps` folder renamed into place, stands for
//! writes of every file (and symlink) under it; the folder itself and the
//! folders in it don't count as written. The archive takes those files like
//! any written file. The rename's source, like Vite's `deps_temp_*` folder,
//! isn't recorded, but a path that is gone after the run and that the task
//! wrote in counts as written: the task made or removed it. It is
//! fingerprinted as missing, which a later run must match either way, so this
//! only tells it apart from a path the task looked for and didn't find. A
//! write record on a path that is a folder, from opening it with write
//! access, is ignored. A mkdir record counts only for a folder that is empty
//! after the run, whether or not the call made it, since it may have been
//! there already: what any other folder holds decides whether it is
//! rebuildable.

use std::{cmp::Reverse, fs, io};

use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

use super::fingerprint::PathFingerprint;

/// The paths a task wrote, the empty folders it made, and the folders
/// holding only what it can rebuild, relative to the workspace root.
#[derive(Debug, Default)]
pub struct Rebuildable {
    written: FxHashSet<RelativePathBuf>,
    /// The folders the task made, or tried to make, inside the workspace.
    mkdirs: FxHashSet<RelativePathBuf>,
    /// The `mkdirs` that are empty folders.
    empty_folders: FxHashSet<RelativePathBuf>,
    /// The rebuildable folders, `empty_folders` among them.
    folders: FxHashSet<RelativePathBuf>,
    /// The folders above the written paths.
    written_in: FxHashSet<RelativePathBuf>,
}

impl Rebuildable {
    /// Takes the `written` paths and the `rename_targets` as written, with the
    /// files under a renamed folder in place of the folder, by looking under
    /// `workspace_root`, which must be done after the run, and the `mkdirs`.
    /// No folder is rebuildable, and none of the `mkdirs` is an empty folder,
    /// until [`Self::with_folders`] or [`Self::with_empty_folders`] finds them.
    pub fn new(
        written: FxHashSet<RelativePathBuf>,
        rename_targets: FxHashSet<RelativePathBuf>,
        mkdirs: FxHashSet<RelativePathBuf>,
        workspace_root: &AbsolutePath,
    ) -> Self {
        let written = files_written(written, rename_targets, workspace_root);
        let mut written_in = FxHashSet::default();
        for path in &written {
            let mut dir = path.parent();
            // A folder already reached from another path has its parents too.
            while let Some(parent) = dir
                && !parent.as_str().is_empty()
                && !written_in.contains(parent)
            {
                written_in.insert(parent.to_relative_path_buf());
                dir = parent.parent();
            }
        }
        Self {
            written,
            mkdirs,
            empty_folders: FxHashSet::default(),
            folders: FxHashSet::default(),
            written_in,
        }
    }

    /// Finds the rebuildable folders, the empty folders the task made among
    /// them, by listing the folders above the written paths and the folders
    /// the task made under `workspace_root`, which must be done after the run.
    /// Only fingerprints of the paths the task read ask about folders, so a
    /// run without such reads takes [`Self::with_empty_folders`] instead.
    #[must_use]
    pub fn with_folders(mut self, workspace_root: &AbsolutePath) -> Self {
        (self.folders, self.empty_folders) =
            find_folders(&self.written, &self.mkdirs, workspace_root);
        self
    }

    /// Finds the empty folders the task made, which the archive takes, by
    /// listing the folders it made under `workspace_root`, which must be done
    /// after the run, without climbing to the folders above them.
    #[must_use]
    pub fn with_empty_folders(mut self, workspace_root: &AbsolutePath) -> Self {
        self.empty_folders = self
            .mkdirs
            .iter()
            .filter(|dir| is_empty_folder(dir, workspace_root))
            .cloned()
            .collect();
        self
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

    /// Whether a later run may find `path`, which a read found as
    /// `fingerprint`, missing: the task wrote it, or it is gone after the run
    /// and the task wrote in it. A write record on a folder, from opening it
    /// with write access, is ignored. Only a written path that the read found
    /// as a folder is stat'd under `workspace_root`, to tell a folder from a
    /// link the task made to one.
    pub fn may_be_missing(
        &self,
        path: &RelativePath,
        fingerprint: &PathFingerprint,
        workspace_root: &AbsolutePath,
    ) -> bool {
        match fingerprint {
            PathFingerprint::NotFound => {
                self.written.contains(path) || self.written_in.contains(path)
            }
            PathFingerprint::FileContentHash(_) => self.written.contains(path),
            PathFingerprint::Folder(_) => {
                self.written.contains(path)
                    && !fs::symlink_metadata(workspace_root.join(path))
                        .is_ok_and(|meta| meta.is_dir())
            }
        }
    }

    /// Whether `path` is an empty folder the task made, or a folder holding
    /// only what the task can rebuild.
    pub fn is_rebuildable_folder(&self, path: &RelativePath) -> bool {
        self.folders.contains(path)
    }

    /// Whether the task can rebuild nothing, like most lint and test tasks, so
    /// no entry of a listing is rebuildable.
    pub fn is_empty(&self) -> bool {
        self.written.is_empty() && self.folders.is_empty()
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
/// are, recursively. A symlink to a folder is a file here, as in [`holds`].
/// Entries that can't be read or named are left out. A written path that is a
/// folder, from opening it with write access, stays without a stat: nothing
/// under it counts, folder entries are checked against the rebuildable
/// folders, and the archive skips folders, so only a read of it has to tell
/// (see [`Rebuildable::may_be_missing`]).
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

/// Finds the folders holding only what the task can rebuild, and the empty
/// folders the task made among them. The folders above a written path are
/// candidates, listed once, deepest first, so a folder's subfolders are
/// decided before it; an empty one is rebuildable if the task made it. A
/// folder the task made that isn't a candidate holds no written path, so only
/// being empty makes it rebuildable, and then the folders above it are
/// candidates too. Climbing stops at the first folder that isn't rebuildable,
/// since no folder above it can be, and at the workspace root. A folder that
/// is gone plays no part: its parent is decided by the entries it holds.
fn find_folders(
    written: &FxHashSet<RelativePathBuf>,
    mkdirs: &FxHashSet<RelativePathBuf>,
    workspace_root: &AbsolutePath,
) -> (FxHashSet<RelativePathBuf>, FxHashSet<RelativePathBuf>) {
    let mut candidates = FxHashSet::default();
    for path in written {
        add_parents(path, &mut candidates);
    }
    let mut empty_folders = FxHashSet::default();
    let mut made: Vec<&RelativePath> = mkdirs
        .iter()
        .map(RelativePathBuf::as_relative_path)
        .filter(|dir| !candidates.contains(dir))
        .collect();
    sort_deepest_first(&mut made);
    for dir in made {
        // A folder holding an empty folder found here is a candidate by now.
        if !candidates.contains(dir) && is_empty_folder(dir, workspace_root) {
            add_parents(dir, &mut candidates);
            empty_folders.insert(dir.to_relative_path_buf());
        }
    }
    let mut candidates: Vec<&RelativePath> = candidates.into_iter().collect();
    sort_deepest_first(&mut candidates);

    let mut folders = empty_folders.clone();
    let mut above_unrebuildable = FxHashSet::default();
    for dir in candidates {
        let contents = if above_unrebuildable.contains(dir) {
            Some(Holds::Other)
        } else {
            holds(dir, workspace_root, written, &folders)
        };
        match contents {
            Some(Holds::OnlyRebuildable) => {
                folders.insert(dir.to_relative_path_buf());
            }
            Some(Holds::Nothing) if mkdirs.contains(dir) => {
                folders.insert(dir.to_relative_path_buf());
                empty_folders.insert(dir.to_relative_path_buf());
            }
            Some(Holds::Nothing | Holds::Other) => {
                if let Some(parent) = dir.parent() {
                    above_unrebuildable.insert(parent);
                }
            }
            None => {}
        }
    }
    (folders, empty_folders)
}

/// Adds the folders above `path` inside the workspace to `candidates`, up to
/// one that is there already and has its parents there too.
fn add_parents<'a>(path: &'a RelativePath, candidates: &mut FxHashSet<&'a RelativePath>) {
    let mut dir = path.parent();
    while let Some(parent) = dir
        && !parent.as_str().is_empty()
        && candidates.insert(parent)
    {
        dir = parent.parent();
    }
}

/// Sorts `dirs` so that each comes before the folders above it.
fn sort_deepest_first(dirs: &mut [&RelativePath]) {
    dirs.sort_unstable_by_key(|dir| Reverse(dir.as_str().matches('/').count()));
}

/// What a folder holds, `.DS_Store` aside, which doesn't count, as in
/// directory listing fingerprints.
enum Holds {
    Nothing,
    /// At least one entry, each a file (or symlink) the task wrote or a
    /// rebuildable folder.
    OnlyRebuildable,
    /// Anything else, or entries that can't be read or named.
    Other,
}

/// What `dir` holds, with `written` and `folders` deciding which entries are
/// rebuildable, or `None` when `dir` isn't a folder anymore. An empty folder
/// is rebuildable only if the task made it, so a hit recreates it.
fn holds(
    dir: &RelativePath,
    workspace_root: &AbsolutePath,
    written: &FxHashSet<RelativePathBuf>,
    folders: &FxHashSet<RelativePathBuf>,
) -> Option<Holds> {
    let entries = match fs::read_dir(workspace_root.join(dir)) {
        Ok(entries) => entries,
        // Gone, or not a folder anymore: it plays no part.
        Err(err)
            if matches!(err.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) =>
        {
            return None;
        }
        Err(_) => return Some(Holds::Other),
    };
    let mut holds = Holds::Nothing;
    for entry in entries {
        let Ok(entry) = entry else {
            return Some(Holds::Other);
        };
        let name = entry.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let (Some(name), Ok(file_type)) = (name.to_str(), entry.file_type()) else {
            return Some(Holds::Other);
        };
        let Ok(name) = RelativePathBuf::new(name) else {
            return Some(Holds::Other);
        };
        if !is_rebuildable(&dir.join(name), file_type.is_dir(), written, folders) {
            return Some(Holds::Other);
        }
        holds = Holds::OnlyRebuildable;
    }
    Some(holds)
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

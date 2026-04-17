//! Normalize raw fspy path accesses into workspace-relative, filtered form.

use std::collections::hash_map::Entry;

use fspy::{AccessMode, PathAccessIterable};
use rustc_hash::FxHashSet;
use vite_path::{AbsolutePath, RelativePathBuf};

use crate::collections::HashMap;

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Tracked file accesses from fspy, normalized to workspace-relative paths.
#[derive(Default, Debug)]
pub struct TrackedPathAccesses {
    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: FxHashSet<RelativePathBuf>,
}

impl TrackedPathAccesses {
    /// Build from fspy's raw iterable by stripping the workspace prefix,
    /// normalizing `..` components, and filtering against the negative globs.
    pub fn from_raw(
        raw: &PathAccessIterable,
        workspace_root: &AbsolutePath,
        resolved_negatives: &[wax::Glob<'static>],
    ) -> Self {
        let mut accesses = Self::default();
        for access in raw.iter() {
            // Strip workspace root, clean `..` components, and filter in one pass.
            // fspy may report paths like `packages/sub-pkg/../shared/dist/output.js`.
            let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
                let Ok(stripped_path) = strip_result else {
                    return None;
                };
                normalize_tracked_workspace_path(stripped_path, resolved_negatives)
            });

            let Some(relative_path) = relative_path else {
                continue;
            };

            if access.mode.contains(AccessMode::READ) {
                accesses
                    .path_reads
                    .entry(relative_path.clone())
                    .or_insert(PathRead { read_dir_entries: false });
            }
            if access.mode.contains(AccessMode::WRITE) {
                accesses.path_writes.insert(relative_path.clone());
            }
            if access.mode.contains(AccessMode::READ_DIR) {
                match accesses.path_reads.entry(relative_path) {
                    Entry::Occupied(mut occupied) => {
                        occupied.get_mut().read_dir_entries = true;
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(PathRead { read_dir_entries: true });
                    }
                }
            }
        }
        accesses
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "fspy strip_path_prefix exposes std::path::Path; convert to RelativePathBuf immediately"
)]
fn normalize_tracked_workspace_path(
    stripped_path: &std::path::Path,
    resolved_negatives: &[wax::Glob<'static>],
) -> Option<RelativePathBuf> {
    // On Windows, paths are possible to be still absolute after stripping the workspace root.
    // For example: c:\workspace\subdir\c:\workspace\subdir
    // Just ignore those accesses.
    let relative = RelativePathBuf::new(stripped_path).ok()?;

    // Clean `..` components — fspy may report paths like
    // `packages/sub-pkg/../shared/dist/output.js`. Normalize them for
    // consistent behavior across platforms and clean user-facing messages.
    let relative = relative.clean().ok()?;

    // Skip .git directory accesses (workaround for tools like oxlint)
    if relative.as_path().strip_prefix(".git").is_ok() {
        return None;
    }

    if !resolved_negatives.is_empty()
        && resolved_negatives.iter().any(|neg| wax::Program::is_match(neg, relative.as_str()))
    {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn malformed_windows_drive_path_after_workspace_strip_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let relative_path =
            normalize_tracked_workspace_path(std::path::Path::new(r"foo\C:\bar"), &[]);
        assert!(relative_path.is_none());
    }
}

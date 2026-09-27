//! Normalize raw fspy path accesses into workspace-relative, filtered form.
//!
//! User-configured negative globs are NOT applied here. They are applied later,
//! separately for reads (input config) and writes (output config), since those
//! two configs are independent.
#![cfg(fspy)]

use std::collections::hash_map::Entry;

use fspy::{AccessMode, PathAccessIterable};
use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePathBuf};

use super::fingerprint::PathRead;
use crate::collections::HashMap;

/// Tracked file accesses from fspy, normalized to workspace-relative paths.
#[derive(Default, Debug)]
pub struct TrackedPathAccesses {
    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: FxHashSet<RelativePathBuf>,
}

impl TrackedPathAccesses {
    /// Build from fspy's raw iterable by stripping the workspace prefix and
    /// normalizing `..` components. Paths outside the workspace, including
    /// ones that climb out of it with `..`, and `.git/*` paths are skipped.
    /// User-configured negatives are applied by the caller (see module docs).
    pub fn from_raw(raw: &PathAccessIterable, workspace_root: &AbsolutePath) -> Self {
        let mut accesses = Self::default();
        for access in raw.iter() {
            // Strip workspace root and clean `..` components in one pass.
            // fspy may report paths like `packages/sub-pkg/../shared/dist/output.js`.
            let relative_path = access.path.strip_path_prefix(workspace_root, |strip_result| {
                let Ok(stripped_path) = strip_result else {
                    return None;
                };
                normalize_tracked_workspace_path(stripped_path)
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
fn normalize_tracked_workspace_path(stripped_path: &std::path::Path) -> Option<RelativePathBuf> {
    // On Windows, paths are possible to be still absolute after stripping the workspace root.
    // For example: c:\workspace\subdir\c:\workspace\subdir
    // Just ignore those accesses.
    let relative = RelativePathBuf::new(stripped_path).ok()?;

    // Clean `..` components — fspy may report paths like
    // `packages/sub-pkg/../shared/dist/output.js`. Normalize them for
    // consistent behavior across platforms and clean user-facing messages.
    let relative = relative.clean().ok()?;

    // Skip paths that climb out of the workspace, such as `pkg/../../file`,
    // like paths that don't start with the workspace root.
    if relative.has_parent_dir_component() {
        return None;
    }

    // Skip .git directory accesses (workaround for tools like oxlint)
    if relative.as_path().strip_prefix(".git").is_ok() {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(
        clippy::disallowed_types,
        reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
    )]
    fn path_that_climbs_out_of_the_workspace_is_ignored() {
        let outside = normalize_tracked_workspace_path(std::path::Path::new("pkg/../../file.txt"));
        assert!(outside.is_none());
        let inside =
            normalize_tracked_workspace_path(std::path::Path::new("pkg/../shared/file.txt"));
        assert_eq!(inside.unwrap().as_str(), "shared/file.txt");
    }

    #[cfg(windows)]
    #[test]
    fn malformed_windows_drive_path_after_workspace_strip_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let relative_path = normalize_tracked_workspace_path(std::path::Path::new(r"foo\C:\bar"));
        assert!(relative_path.is_none());
    }
}

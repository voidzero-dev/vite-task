//! Normalize raw fspy path accesses into workspace-relative, filtered form.
//!
//! User-configured negative globs are NOT applied here. They are applied later,
//! separately for reads (input config) and writes (output config), since those
//! two configs are independent.
#![cfg(fspy)]

use fspy::{IpcPath, PathAccess, PathAccessIterable};
use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

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
    /// normalizing `..` components. `.git/*` paths, pnpm's workspace state
    /// files and paths in the task cache at `cache_root` are skipped.
    /// User-configured negatives are applied by the caller (see module docs).
    pub fn from_raw(
        raw: &PathAccessIterable,
        workspace_root: &AbsolutePath,
        cache_root: &AbsolutePath,
    ) -> Self {
        // The task cache, when it's inside the workspace. A cache at the
        // workspace root itself isn't skipped, since that would skip every
        // access.
        let cache_root = cache_root
            .strip_prefix(workspace_root)
            .ok()
            .flatten()
            .filter(|cache_root| !cache_root.as_str().is_empty());

        // Strip workspace root and clean `..` components in one pass.
        // fspy may report paths like `packages/sub-pkg/../shared/dist/output.js`.
        let relative_path = |path: &IpcPath| {
            path.strip_path_prefix(workspace_root, |strip_result| {
                normalize_tracked_workspace_path(strip_result.ok()?, cache_root.as_deref())
            })
        };

        let mut accesses = Self::default();
        for access in raw.iter() {
            match access {
                PathAccess::Read(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.add_read(path);
                    }
                }
                PathAccess::Write(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.path_writes.insert(path);
                    }
                }
                PathAccess::ReadWrite(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.add_read(path.clone());
                        accesses.path_writes.insert(path);
                    }
                }
                PathAccess::ReadDir(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.add_read(path).read_dir_entries = true;
                    }
                }
            }
        }
        accesses
    }

    fn add_read(&mut self, path: RelativePathBuf) -> &mut PathRead {
        self.path_reads.entry(path).or_insert(PathRead { read_dir_entries: false })
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "fspy strip_path_prefix exposes std::path::Path; convert to RelativePathBuf immediately"
)]
fn normalize_tracked_workspace_path(
    stripped_path: &std::path::Path,
    cache_root: Option<&RelativePath>,
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

    if is_pnpm_workspace_state(&relative) {
        return None;
    }

    // Skip the task cache. vp rewrites it after every run, so a task that
    // reads it, like a tool crawling the project, would never hit.
    if cache_root.is_some_and(|cache_root| relative.strip_prefix(cache_root).is_some()) {
        return None;
    }

    Some(relative)
}

/// Whether `relative` is a `node_modules/.pnpm-workspace-state-*.json` file,
/// pnpm's record of its dependency check before `pnpm run` and `pnpm exec`.
/// pnpm rewrites it with a new timestamp whenever it rechecks dependencies,
/// and it holds absolute project paths. Whether the check passes doesn't
/// change what a cached task outputs.
fn is_pnpm_workspace_state(relative: &RelativePathBuf) -> bool {
    let path = relative.as_str();
    let (dir, file_name) = path.rsplit_once('/').unwrap_or(("", path));
    (dir == "node_modules" || dir.ends_with("/node_modules"))
        && file_name
            .strip_prefix(".pnpm-workspace-state-")
            .is_some_and(|rest| rest.strip_suffix(".json").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_workspace_state_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let normalize =
            |path: &str| normalize_tracked_workspace_path(std::path::Path::new(path), None);
        assert!(normalize("node_modules/.pnpm-workspace-state-v1.json").is_none());
        assert!(normalize("node_modules/.pnpm-workspace-state-v2.json").is_none());
        assert!(normalize("packages/a/node_modules/.pnpm-workspace-state-v1.json").is_none());
        assert!(normalize("node_modules/.modules.yaml").is_some());
        assert!(normalize(".pnpm-workspace-state-v1.json").is_some());
        assert!(normalize("node_modules/a/.pnpm-workspace-state-v1.json").is_some());
    }

    #[test]
    fn task_cache_is_ignored() {
        let cache_root = RelativePathBuf::new("node_modules/.vite/task-cache").unwrap();
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let normalize = |path: &str| {
            normalize_tracked_workspace_path(std::path::Path::new(path), Some(&cache_root))
        };
        assert!(normalize("node_modules/.vite/task-cache").is_none());
        assert!(normalize("node_modules/.vite/task-cache/v21/cache.db").is_none());
        assert!(normalize("node_modules/.vite").is_some());
        assert!(normalize("node_modules/.vite/task-cache-old/cache.db").is_some());
        assert!(normalize("node_modules/.vite/deps/react.js").is_some());
    }

    #[cfg(windows)]
    #[test]
    fn malformed_windows_drive_path_after_workspace_strip_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let relative_path =
            normalize_tracked_workspace_path(std::path::Path::new(r"foo\C:\bar"), None);
        assert!(relative_path.is_none());
    }
}

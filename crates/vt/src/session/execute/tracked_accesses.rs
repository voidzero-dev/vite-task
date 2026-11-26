//! Normalize raw fspy path accesses into workspace-relative, filtered form.
//!
//! User-configured negative globs are NOT applied here. They are applied later,
//! separately for reads (input config) and writes (output config), since those
//! two configs are independent.
#![cfg(fspy)]

use fspy::{IpcPath, PathAccess, PathAccessIterable};
use rustc_hash::FxHashSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

use super::{fingerprint::PathRead, ignored_paths::is_ignored_workspace_path};
use crate::collections::HashMap;

/// Tracked file accesses from fspy, normalized to workspace-relative paths.
#[derive(Default, Debug)]
pub struct TrackedPathAccesses {
    /// Tracked path reads
    pub path_reads: HashMap<RelativePathBuf, PathRead>,

    /// Tracked path writes
    pub path_writes: FxHashSet<RelativePathBuf>,

    /// Tracked rename targets: the new paths of renamed files and folders
    pub rename_targets: FxHashSet<RelativePathBuf>,

    /// Tracked mkdirs: the folders the task made, or tried to make
    pub mkdirs: FxHashSet<RelativePathBuf>,
}

impl TrackedPathAccesses {
    /// Build from fspy's raw iterable by resolving `..` components and then
    /// stripping the workspace prefix, so paths outside the workspace are
    /// skipped. Built-in ignored paths (see [`super::ignored_paths`]),
    /// including `.git/*`, pnpm's workspace state files, Vite's `.vite-temp`,
    /// `.DS_Store`, and paths in the task cache at `cache_root`, are skipped
    /// too.
    /// User-configured negatives are applied by the caller (see module docs).
    pub fn from_raw(
        raw: &PathAccessIterable,
        workspace_root: &AbsolutePath,
        cache_root: Option<&RelativePath>,
    ) -> Self {
        let relative_path =
            |path: &IpcPath| tracked_workspace_path(path, workspace_root, cache_root);

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
                PathAccess::RenameTarget(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.rename_targets.insert(path);
                    }
                }
                PathAccess::Mkdir(path) => {
                    if let Some(path) = relative_path(path) {
                        accesses.mkdirs.insert(path);
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

/// The workspace-relative path of the tracked `path`, or `None` if it's
/// outside the workspace or skipped (see [`normalize_tracked_workspace_path`]).
fn tracked_workspace_path(
    path: &IpcPath,
    workspace_root: &AbsolutePath,
    cache_root: Option<&RelativePath>,
) -> Option<RelativePathBuf> {
    // Resolve `..` components before stripping the workspace root. fspy may
    // report paths like `<workspace>/packages/sub-pkg/../shared/dist/output.js`,
    // which is `packages/shared/dist/output.js`, and on Unix reports a relative
    // open of `../site/index.html` from the workspace root as
    // `<workspace>/../site/index.html`, which is outside the workspace. On
    // Windows, cleaning also turns malformed paths like
    // `c:\workspace\subdir\c:\workspace\subdir` into ones outside it.
    let stripped_path = path.clean_and_strip_path_prefix(workspace_root).ok()?;
    normalize_tracked_workspace_path(&stripped_path, cache_root)
}

#[expect(
    clippy::disallowed_types,
    reason = "fspy clean_and_strip_path_prefix returns std::path::PathBuf; convert to RelativePathBuf immediately"
)]
fn normalize_tracked_workspace_path(
    stripped_path: &std::path::Path,
    cache_root: Option<&RelativePath>,
) -> Option<RelativePathBuf> {
    // Ignore paths that aren't portable, like ones with non-UTF-8 names.
    let relative = RelativePathBuf::new(stripped_path).ok()?;

    if is_ignored_workspace_path(&relative, cache_root) {
        return None;
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workspace-relative path of `path` under the workspace root `/repo`
    /// (`C:\repo` on Windows), tracked as fspy reports it: joined to the root,
    /// `..` included.
    fn tracked(path: &str) -> Option<RelativePathBuf> {
        let workspace_root =
            AbsolutePath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).unwrap();
        let path = workspace_root.join(path);
        #[cfg(unix)]
        let ipc_path = <&IpcPath>::from(path.as_path());
        #[cfg(windows)]
        let wide: Vec<u16> = {
            use std::os::windows::ffi::OsStrExt as _;
            path.as_path().as_os_str().encode_wide().collect()
        };
        #[cfg(windows)]
        let ipc_path = IpcPath::from_wide(&wide);
        tracked_workspace_path(ipc_path, workspace_root, None)
    }

    #[test]
    fn dot_dot_is_resolved_before_the_workspace_root_is_stripped() {
        let rel = |path: &str| Some(RelativePathBuf::new(path).unwrap());
        assert_eq!(
            tracked("packages/sub-pkg/../shared/dist/output.js"),
            rel("packages/shared/dist/output.js")
        );
        assert_eq!(tracked("../repo/dist/output.js"), rel("dist/output.js"));
        // A relative open of `../site/index.html` from the workspace root.
        assert_eq!(tracked("../site/index.html"), None);
        assert_eq!(tracked(".."), None);
    }

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
    fn vite_temp_is_ignored() {
        #[expect(
            clippy::disallowed_types,
            reason = "normalize_tracked_workspace_path requires std::path::Path for fspy strip_path_prefix output"
        )]
        let normalize =
            |path: &str| normalize_tracked_workspace_path(std::path::Path::new(path), None);
        assert!(normalize("node_modules/.vite-temp").is_none());
        assert!(normalize("node_modules/.vite-temp/vite.config.ts.timestamp-1.mjs").is_none());
        assert!(normalize("packages/void/node_modules/.vite-temp").is_none());
        assert!(normalize("packages/void/node_modules/.vite-temp/sub/config.mjs").is_none());
        assert!(normalize(".vite-temp/config.mjs").is_some());
        assert!(normalize("node_modules/.vite-temp-other").is_some());
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
        assert!(tracked(r"foo\C:\bar").is_none());
    }
}

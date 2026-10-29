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

    /// Tracked rename targets: the new paths of renamed files and folders
    pub rename_targets: FxHashSet<RelativePathBuf>,
}

impl TrackedPathAccesses {
    /// Build from fspy's raw iterable by resolving `..` components and then
    /// stripping the workspace prefix, so paths outside the workspace are
    /// skipped. `.git/*` paths, pnpm's workspace state files and paths in the
    /// task cache at `cache_root` are skipped too.
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

        let relative_path =
            |path: &IpcPath| tracked_workspace_path(path, workspace_root, cache_root.as_deref());

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

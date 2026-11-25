//! Built-in workspace-relative paths ignored by automatic tracking, directory
//! listing fingerprints, and rebuildable-folder scans.
//!
//! Direct fspy path accesses (`tracked_accesses`), directory listing entries
//! ([`super::fingerprint`]), and rebuildable folder scans
//! ([`super::rebuildable`]) all filter through this module so they agree on
//! which volatile or internal workspace paths never participate in cache keys,
//! inferred inputs, or outputs.

use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};

/// The task cache root (the parent of the schema-version folder `cache_dir`),
/// relative to `workspace_root`, or `None` when it is outside the workspace or
/// is the workspace root itself (skipping the root would skip every access).
#[must_use]
pub fn relative_cache_root(
    cache_dir: &AbsolutePath,
    workspace_root: &AbsolutePath,
) -> Option<RelativePathBuf> {
    let cache_root = cache_dir.parent().unwrap_or(cache_dir);
    cache_root
        .strip_prefix(workspace_root)
        .ok()
        .flatten()
        .filter(|cache_root| !cache_root.as_str().is_empty())
}

/// Built-in ignored entry matcher precomputed for one workspace-relative
/// directory, so per-entry checks in large directories avoid repeating
/// directory-level prefix checks.
pub(super) struct IgnoredDirEntries<'a> {
    /// Every entry in this directory is ignored because the directory itself
    /// lies inside an ignored subtree (root `.git`, `node_modules/.vite-temp`,
    /// or `cache_root`).
    all: bool,
    /// The directory is the workspace root (`""`), where `.git` is ignored.
    is_root: bool,
    /// The directory is a `node_modules` directory, where
    /// `.pnpm-workspace-state-*.json` and `.vite-temp` are ignored.
    is_node_modules: bool,
    /// The entry name of `cache_root` when the directory is its immediate
    /// parent.
    cache_root_name: Option<&'a str>,
}

impl<'a> IgnoredDirEntries<'a> {
    /// Build a matcher for entries directly inside `dir`.
    #[must_use]
    pub(super) fn for_dir(dir: &RelativePath, cache_root: Option<&'a RelativePath>) -> Self {
        let dir_str = dir.as_str();
        let all = is_under_root_git(dir)
            || is_under_node_modules_vite_temp(dir_str)
            || cache_root
                .is_some_and(|root| !root.as_str().is_empty() && dir.strip_prefix(root).is_some());
        let cache_root_name = cache_root
            .filter(|root| !root.as_str().is_empty() && root.parent() == Some(dir))
            .and_then(RelativePath::file_name);
        Self {
            all,
            is_root: dir_str.is_empty(),
            is_node_modules: is_node_modules_dir(dir_str),
            cache_root_name,
        }
    }

    /// Whether this directory has any built-in ignored entries beyond
    /// `.DS_Store`, which raw directory listing already skips.
    #[must_use]
    pub(super) const fn has_non_ds_store_rules(&self) -> bool {
        self.all || self.is_root || self.is_node_modules || self.cache_root_name.is_some()
    }

    /// Whether the entry `name` in this directory is a built-in ignored path:
    ///
    /// - `.DS_Store` (Finder metadata in any folder on macOS)
    /// - `.git` at the workspace root (workaround for tools like oxlint, and
    ///   keeps worktrees/clones/exports with different `.git` layouts from
    ///   busting the cache)
    /// - `node_modules/.pnpm-workspace-state-*.json` (pnpm's dependency check
    ///   record before `pnpm run`/`pnpm exec`, rewritten with timestamps and
    ///   absolute paths)
    /// - `node_modules/.vite-temp` and anything under it (Vite's transient
    ///   compiled `vite.config.*` folder)
    /// - `cache_root` and anything under it (the task cache `vp` rewrites
    ///   after every run)
    #[must_use]
    pub(super) fn is_ignored(&self, name: &str) -> bool {
        self.all
            || name == ".DS_Store"
            || (self.is_root && name == ".git")
            || (self.is_node_modules
                && (name == ".vite-temp" || is_pnpm_workspace_state_file(name)))
            || self.cache_root_name == Some(name)
    }
}

/// Whether `path`, relative to the workspace root, is a built-in ignored path.
#[cfg_attr(
    all(not(fspy), not(test)),
    expect(dead_code, reason = "only called when filtering fspy path accesses")
)]
#[must_use]
pub(super) fn is_ignored_workspace_path(
    path: &RelativePath,
    cache_root: Option<&RelativePath>,
) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    IgnoredDirEntries::for_dir(dir, cache_root).is_ignored(name)
}

fn is_under_root_git(dir: &RelativePath) -> bool {
    dir.as_path().strip_prefix(".git").is_ok()
}

fn is_node_modules_dir(dir: &str) -> bool {
    dir == "node_modules" || dir.ends_with("/node_modules")
}

fn is_pnpm_workspace_state_file(name: &str) -> bool {
    name.strip_prefix(".pnpm-workspace-state-")
        .is_some_and(|rest| rest.strip_suffix(".json").is_some())
}

fn is_under_node_modules_vite_temp(dir: &str) -> bool {
    dir == "node_modules/.vite-temp"
        || dir.starts_with("node_modules/.vite-temp/")
        || dir.ends_with("/node_modules/.vite-temp")
        || dir.contains("/node_modules/.vite-temp/")
}

#[cfg(test)]
mod tests {
    use vt_path::{AbsolutePath, RelativePathBuf};

    use super::*;

    fn rel(path: &str) -> RelativePathBuf {
        RelativePathBuf::new(path).unwrap()
    }

    fn assert_both(path: &str, cache_root: Option<&RelativePath>, expected: bool) {
        let relative = rel(path);
        assert_eq!(
            is_ignored_workspace_path(&relative, cache_root),
            expected,
            "workspace path `{path}`"
        );
        if let (Some(parent), Some(name)) = (relative.parent(), relative.file_name()) {
            assert_eq!(
                IgnoredDirEntries::for_dir(parent, cache_root).is_ignored(name),
                expected,
                "dir entry `{name}` in `{}`",
                parent.as_str()
            );
        }
    }

    #[test]
    fn ds_store_is_ignored() {
        assert_both(".DS_Store", None, true);
        assert_both("packages/a/.DS_Store", None, true);
        assert_both("node_modules/.DS_Store", None, true);
        assert_both(".DS_Store.bak", None, false);
    }

    #[test]
    fn root_git_is_ignored() {
        assert_both(".git", None, true);
        assert_both(".git/HEAD", None, true);
        assert_both(".git/objects/ab/cd", None, true);
        assert_both(".gitignore", None, false);
        assert_both("packages/a/.git", None, false);
    }

    #[test]
    fn pnpm_workspace_state_is_ignored() {
        assert_both("node_modules/.pnpm-workspace-state-v1.json", None, true);
        assert_both("node_modules/.pnpm-workspace-state-v2.json", None, true);
        assert_both("packages/a/node_modules/.pnpm-workspace-state-v1.json", None, true);
        assert_both("node_modules/.modules.yaml", None, false);
        assert_both(".pnpm-workspace-state-v1.json", None, false);
        assert_both("node_modules/a/.pnpm-workspace-state-v1.json", None, false);
        assert_both("my_node_modules/.pnpm-workspace-state-v1.json", None, false);
    }

    #[test]
    fn vite_temp_is_ignored() {
        assert_both("node_modules/.vite-temp", None, true);
        assert_both("node_modules/.vite-temp/vite.config.ts.timestamp-123.mjs", None, true);
        assert_both("packages/void/node_modules/.vite-temp", None, true);
        assert_both("packages/void/node_modules/.vite-temp/sub/file.mjs", None, true);
        assert_both("packages/void/node_modules/pkg/node_modules/.vite-temp", None, true);
        assert_both(".vite-temp", None, false);
        assert_both(".vite-temp/file.mjs", None, false);
        assert_both("node_modules/.vite-temp-other", None, false);
        assert_both("node_modules/a/.vite-temp", None, false);
        assert_both("my_node_modules/.vite-temp", None, false);
        assert_both("node_modules/.vite", None, false);
        assert_both("node_modules/.vite/deps/react.js", None, false);
    }

    #[test]
    fn task_cache_is_ignored() {
        let cache_root = rel("node_modules/.vite/task-cache");
        assert_both("node_modules/.vite/task-cache", Some(&cache_root), true);
        assert_both("node_modules/.vite/task-cache/v24/cache.db", Some(&cache_root), true);
        assert_both("node_modules/.vite", Some(&cache_root), false);
        assert_both("node_modules/.vite/task-cache-old", Some(&cache_root), false);
        assert_both("node_modules/.vite/task-cache-old/cache.db", Some(&cache_root), false);
        assert_both("node_modules/.vite/deps/react.js", Some(&cache_root), false);

        let top_level_cache = rel("cache");
        assert_both("cache", Some(&top_level_cache), true);
        assert_both("cache/v24/cache.db", Some(&top_level_cache), true);
        assert_both("cache-old", Some(&top_level_cache), false);
    }

    #[test]
    fn workspace_root_is_never_ignored() {
        let empty = rel("");
        assert!(!is_ignored_workspace_path(&empty, None));
        assert!(!is_ignored_workspace_path(&empty, Some(&empty)));
    }

    #[test]
    fn non_special_dir_has_no_non_ds_store_rules() {
        let cache_root = rel("node_modules/.vite/task-cache");
        assert!(
            !IgnoredDirEntries::for_dir(&rel("src"), Some(&cache_root)).has_non_ds_store_rules()
        );
        assert!(
            !IgnoredDirEntries::for_dir(&rel("packages/a/src"), Some(&cache_root))
                .has_non_ds_store_rules()
        );
        assert!(IgnoredDirEntries::for_dir(&rel(""), Some(&cache_root)).has_non_ds_store_rules());
        assert!(
            IgnoredDirEntries::for_dir(&rel("node_modules"), Some(&cache_root))
                .has_non_ds_store_rules()
        );
        assert!(
            IgnoredDirEntries::for_dir(&rel("node_modules/.vite"), Some(&cache_root))
                .has_non_ds_store_rules()
        );
    }

    #[test]
    fn relative_cache_root_drops_outside_and_workspace_root() {
        let workspace_root =
            AbsolutePath::new(if cfg!(windows) { r"C:\repo" } else { "/repo" }).unwrap();
        let default_cache_dir = workspace_root.join("node_modules/.vite/task-cache/v24");
        assert_eq!(
            relative_cache_root(&default_cache_dir, workspace_root),
            Some(rel("node_modules/.vite/task-cache"))
        );

        let at_root_cache_dir = workspace_root.join("v24");
        assert_eq!(relative_cache_root(&at_root_cache_dir, workspace_root), None);

        let outside_cache_dir =
            AbsolutePath::new(if cfg!(windows) { r"C:\tmp\cache\v24" } else { "/tmp/cache/v24" })
                .unwrap();
        assert_eq!(relative_cache_root(outside_cache_dir, workspace_root), None);
    }
}

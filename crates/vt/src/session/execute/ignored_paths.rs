//! Built-in workspace-relative paths ignored by automatic tracking and
//! directory listing fingerprints.
//!
//! Both direct fspy path accesses (`tracked_accesses`) and directory listing
//! entries ([`super::fingerprint`]) filter through this module so they agree
//! on which volatile or internal workspace paths never participate in cache
//! keys, inferred inputs, or outputs.

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

/// Whether `path`, relative to the workspace root, is a built-in ignored path.
#[cfg_attr(not(fspy), expect(dead_code, reason = "only called when filtering fspy path accesses"))]
#[must_use]
pub(super) fn is_ignored_workspace_path(
    path: &RelativePath,
    cache_root: Option<&RelativePath>,
) -> bool {
    let path = path.as_str();
    if path.is_empty() {
        return false;
    }
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    is_ignored_entry(dir, name, cache_root)
}

/// Whether the entry `name` in the workspace-relative folder `dir` is a
/// built-in ignored path.
///
/// For any non-empty single-component `name`, this agrees with
/// [`is_ignored_workspace_path`] on `dir.join(name)` without allocating.
#[must_use]
pub(super) fn is_ignored_dir_entry(
    dir: &RelativePath,
    name: &str,
    cache_root: Option<&RelativePath>,
) -> bool {
    is_ignored_entry(dir.as_str(), name, cache_root)
}

fn is_ignored_entry(dir: &str, name: &str, cache_root: Option<&RelativePath>) -> bool {
    // Finder metadata files can appear in any folder on macOS.
    if name == ".DS_Store" || has_component(dir, ".DS_Store") {
        return true;
    }

    // Skip .git directory accesses and root listings (workaround for tools like
    // oxlint, and keeps worktrees/clones/exports with different `.git` layouts
    // from busting the cache).
    if is_root_git(dir, name) {
        return true;
    }

    // pnpm's record of its dependency check before `pnpm run` and `pnpm exec`
    // (`node_modules/.pnpm-workspace-state-*.json`). pnpm rewrites it with a
    // new timestamp whenever it rechecks dependencies, and it holds absolute
    // project paths. Whether the check passes doesn't change what a cached
    // task outputs.
    if is_node_modules_dir(dir) && is_pnpm_workspace_state_file(name) {
        return true;
    }

    // Vite's transient compiled config folder (`node_modules/.vite-temp`).
    // Vite writes timestamped `.mjs` files here while loading `vite.config.*`
    // and leaves the directory behind, which otherwise causes spurious misses
    // for any task that lists `node_modules` or walks into it.
    if (is_node_modules_dir(dir) && name == ".vite-temp") || is_under_node_modules_vite_temp(dir) {
        return true;
    }

    // Skip the task cache. vp rewrites it after every run, so a task that
    // reads or lists it, like a tool crawling the project, would never hit.
    if cache_root.is_some_and(|cache_root| is_at_or_under(dir, name, cache_root.as_str())) {
        return true;
    }

    false
}

fn has_component(dir: &str, target: &str) -> bool {
    !dir.is_empty() && dir.split('/').any(|component| component == target)
}

fn is_root_git(dir: &str, name: &str) -> bool {
    (dir.is_empty() && name == ".git") || dir == ".git" || dir.starts_with(".git/")
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

fn is_at_or_under(dir: &str, name: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    if dir == root || dir.strip_prefix(root).is_some_and(|rest| rest.starts_with('/')) {
        return true;
    }
    root.rsplit_once('/').unwrap_or(("", root)) == (dir, name)
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
        if let Some(parent) = relative.parent() {
            let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
            assert_eq!(
                is_ignored_dir_entry(parent, name, cache_root),
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

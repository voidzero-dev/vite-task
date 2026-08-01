//! Version control's opinion about a path, used to break read-first overlaps.
//!
//! When a task reads a path and then rewrites it, access mechanics cannot say
//! whether that path is a source the task fixed up or state the task manages: a
//! formatter rewriting a source and a linter rewriting its own cache look
//! identical. Version control can. A tracked file the task rewrote is a modified
//! input; an ignored one is derived state.
//!
//! This is a proxy, not a definition. Gitignore answers "is this committed",
//! which is not quite "is this derived" — Astro's `.astro/settings.json` is
//! user-authored preferences inside an ignored directory. Explicit input and
//! output globs override the answer, so the proxy only has to be right for paths
//! nobody declared.
//!
//! When there is no repository, every read-first overlap resolves to *input*,
//! apart from the always-derived directories below. That is the safe direction:
//! the task modified something it declared as a dependency, so the run is not
//! cached. Availability degrades, correctness does not.

#![cfg(fspy)]

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use vite_path::{AbsolutePath, RelativePathBuf};

/// Directories that hold derived state in every JavaScript workspace, whether or
/// not a `.gitignore` says so.
///
/// `node_modules` is installed rather than authored, and `.git` is the repository
/// itself. Relying on a `.gitignore` to say so is not enough: a package with no
/// `.gitignore` of its own — or a workspace that is not a repository at all —
/// would classify a tool's own cache under `node_modules` as a modified input and
/// then never cache. Vitest keeps `node_modules/.vite/vitest/*/results.json`,
/// which it reads and then rewrites on every run.
const ALWAYS_IGNORED: &[&str] = &["node_modules", ".git"];

/// Gitignore matching rooted at the workspace.
pub struct WorkspaceGitignore {
    matcher: Option<Gitignore>,
}

impl WorkspaceGitignore {
    /// Build a matcher for the workspace root.
    ///
    /// Collects `.gitignore` at the root plus `.git/info/exclude`. Nested
    /// `.gitignore` files inside subdirectories are picked up by the builder as
    /// it walks the ones it is given, and a missing file is simply no rules.
    ///
    /// A malformed pattern is not fatal. Failing the whole task because one line
    /// of a `.gitignore` is unparsable would be worse than treating that line as
    /// absent, and the fallback direction is safe: fewer ignore rules means more
    /// paths look tracked, which means fewer runs are cached.
    pub fn open(workspace_root: &AbsolutePath) -> Self {
        let mut builder = GitignoreBuilder::new(workspace_root.as_path());
        // Errors here describe individual bad patterns; the partial matcher
        // built from the good ones is still worth having.
        drop(builder.add(workspace_root.join(".gitignore").as_path()));
        drop(builder.add(workspace_root.join(".git/info/exclude").as_path()));
        Self { matcher: builder.build().ok() }
    }

    /// Whether version control ignores this path.
    ///
    /// `false` when there is no matcher at all, which is the input-leaning
    /// answer.
    pub fn is_ignored(&self, path: &RelativePathBuf) -> bool {
        if path
            .as_path()
            .iter()
            .any(|component| ALWAYS_IGNORED.contains(&component.to_string_lossy().as_ref()))
        {
            return true;
        }

        let Some(matcher) = &self.matcher else {
            return false;
        };
        // A directory and a file with the same name match different patterns
        // (`dist/` only matches the directory), so the caller's path is checked
        // both ways rather than guessing.
        matcher.matched_path_or_any_parents(path.as_path(), false).is_ignore()
            || matcher.matched_path_or_any_parents(path.as_path(), true).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vite_path::AbsolutePathBuf;

    use super::*;

    /// A root `dist/` rule must ignore a nested build output. This is the shape
    /// every real monorepo has, and getting it wrong turns every package's
    /// output into a modified input.
    #[test]
    fn root_directory_rule_ignores_nested_output() {
        let temp = TempDir::new().unwrap();
        let root = AbsolutePathBuf::new(temp.path().to_path_buf()).unwrap();
        std::fs::write(root.join(".gitignore").as_path(), "dist/\nnode_modules\n").unwrap();
        std::fs::create_dir_all(root.join("packages/auth/dist").as_path()).unwrap();
        std::fs::write(root.join("packages/auth/dist/index.mjs").as_path(), "x").unwrap();

        let gitignore = WorkspaceGitignore::open(&root);

        assert!(
            gitignore.is_ignored(&RelativePathBuf::new("packages/auth/dist/index.mjs").unwrap()),
            "a root `dist/` rule must cover nested package output"
        );
        assert!(
            !gitignore.is_ignored(&RelativePathBuf::new("packages/auth/src/index.ts").unwrap()),
            "tracked sources must not be reported as ignored"
        );
    }

    /// A workspace with no `.gitignore` must still recognise `node_modules` as
    /// derived. Vitest reads and then rewrites its own
    /// `node_modules/.vite/vitest/*/results.json`, and calling that a modified
    /// input stops the task from ever caching.
    #[test]
    fn node_modules_is_derived_without_any_gitignore() {
        let temp = TempDir::new().unwrap();
        let root = AbsolutePathBuf::new(temp.path().to_path_buf()).unwrap();

        let gitignore = WorkspaceGitignore::open(&root);

        assert!(
            gitignore.is_ignored(
                &RelativePathBuf::new("node_modules/.vite/vitest/abc/results.json").unwrap()
            ),
            "a tool's own cache under node_modules must not become a modified input"
        );
        assert!(
            gitignore
                .is_ignored(&RelativePathBuf::new("packages/auth/node_modules/x/cache").unwrap()),
            "nested node_modules must be covered too"
        );
        assert!(
            !gitignore.is_ignored(&RelativePathBuf::new("src/index.ts").unwrap()),
            "sources must still be tracked when there is no .gitignore"
        );
    }
}

//! Glob-based input file discovery and fingerprinting.
//!
//! This module provides functions to walk glob patterns and compute file hashes
//! for cache invalidation based on explicit input patterns.

use std::{
    collections::BTreeMap,
    fs::File,
    hash::Hasher as _,
    io::{self, Read},
};

use path_clean::PathClean;
#[cfg(test)]
use vite_path::AbsolutePathBuf;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use wax::{Glob, Program as _};

/// A glob pattern resolved to an absolute base directory.
///
/// Uses [`wax::Glob::partition`] to separate the invariant prefix from the
/// wildcard suffix, then resolves the prefix to an absolute path via
/// [`path_clean`] (normalizing components like `..`).
///
/// For example, `../shared/src/**` relative to `/ws/packages/app` resolves to:
/// - `resolved_base`: `/ws/packages/shared/src`
/// - `variant`: `Some(Glob("**"))`
#[expect(clippy::disallowed_types, reason = "path_clean returns std::path::PathBuf")]
pub struct ResolvedGlob {
    resolved_base: std::path::PathBuf,
    variant: Option<Glob<'static>>,
}

impl ResolvedGlob {
    /// Resolve a glob pattern relative to `base_dir`.
    pub fn new(pattern: &str, base_dir: &AbsolutePath) -> anyhow::Result<Self> {
        let glob = Glob::new(pattern)?.into_owned();
        let (base_pathbuf, variant) = glob.partition();
        let base_str = base_pathbuf.to_str().unwrap_or(".");
        let resolved_base = if base_str.is_empty() {
            base_dir.as_path().to_path_buf()
        } else {
            base_dir.join(base_str).as_path().clean()
        };
        Ok(Self { resolved_base, variant: variant.map(Glob::into_owned) })
    }

    /// Walk the filesystem and yield matching file paths.
    #[expect(clippy::disallowed_types, reason = "yields std::path::PathBuf from wax walker")]
    pub fn walk(&self) -> Box<dyn Iterator<Item = std::path::PathBuf> + '_> {
        match &self.variant {
            Some(variant_glob) => Box::new(
                variant_glob
                    .walk(&self.resolved_base)
                    .filter_map(Result::ok)
                    .map(wax::walk::Entry::into_path),
            ),
            None => Box::new(std::iter::once(self.resolved_base.clone())),
        }
    }

    /// Check if an absolute path matches this resolved glob.
    #[expect(clippy::disallowed_types, reason = "matching against std::path::Path")]
    pub fn matches(&self, path: &std::path::Path) -> bool {
        path.strip_prefix(&self.resolved_base).ok().is_some_and(|remainder| {
            self.variant
                .as_ref()
                .map_or(remainder.as_os_str().is_empty(), |v| v.is_match(remainder))
        })
    }
}

/// Compute globbed inputs by walking positive glob patterns and filtering with negative patterns.
///
/// Glob patterns may contain `..` to reference files outside the package directory
/// (e.g., `../shared/src/**` to include a sibling package's source files).
///
/// # Arguments
/// * `base_dir` - The package directory where the task is defined (globs are relative to this)
/// * `workspace_root` - The workspace root for computing relative paths in the result
/// * `positive_globs` - Glob patterns that should match input files
/// * `negative_globs` - Glob patterns that should exclude files from the result
///
/// # Returns
/// A sorted map of relative paths (from `workspace_root`) to their content hashes.
/// Only files are included (directories are skipped).
///
/// # Example
/// ```ignore
/// // For a task defined in `packages/foo/` with inputs: ["src/**/*.ts", "!**/*.test.ts"]
/// let inputs = compute_globbed_inputs(
///     &packages_foo_path,
///     &workspace_root,
///     &["src/**/*.ts".into()].into_iter().collect(),
///     &["**/*.test.ts".into()].into_iter().collect(),
/// )?;
/// // Returns: { "packages/foo/src/index.ts" => 0x1234..., ... }
/// ```
pub fn compute_globbed_inputs(
    base_dir: &AbsolutePath,
    workspace_root: &AbsolutePath,
    positive_globs: &std::collections::BTreeSet<Str>,
    negative_globs: &std::collections::BTreeSet<Str>,
) -> anyhow::Result<BTreeMap<RelativePathBuf, u64>> {
    // If no positive globs, return empty result
    if positive_globs.is_empty() {
        return Ok(BTreeMap::new());
    }

    let negatives: Vec<ResolvedGlob> = negative_globs
        .iter()
        .map(|p| ResolvedGlob::new(p.as_str(), base_dir))
        .collect::<anyhow::Result<_>>()?;

    let mut result = BTreeMap::new();

    for pattern in positive_globs {
        let resolved = ResolvedGlob::new(pattern.as_str(), base_dir)?;

        for absolute_path in resolved.walk() {
            // Skip non-files
            if !absolute_path.is_file() {
                continue;
            }

            // Apply negative patterns
            if negatives.iter().any(|neg| neg.matches(&absolute_path)) {
                continue;
            }

            // Compute path relative to workspace_root for the result
            let Some(relative_to_workspace) = absolute_path
                .strip_prefix(workspace_root.as_path())
                .ok()
                .and_then(|p| RelativePathBuf::new(p).ok())
            else {
                continue; // Skip if path is outside workspace_root
            };

            // Hash file content
            match hash_file_content(&absolute_path) {
                Ok(hash) => {
                    result.insert(relative_to_workspace, hash);
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    // File was deleted between walk and hash, skip it
                }
                Err(err) => {
                    return Err(err.into());
                }
            }
        }
    }

    Ok(result)
}

/// Hash file content using `xxHash3_64`.
#[expect(clippy::disallowed_types, reason = "receives std::path::Path from wax glob walker")]
fn hash_file_content(path: &std::path::Path) -> io::Result<u64> {
    let file = File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let mut hasher = twox_hash::XxHash3_64::default();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn create_test_workspace() -> (TempDir, AbsolutePathBuf, AbsolutePathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(temp_dir.path().to_path_buf()).unwrap();

        // Create package directory structure
        let package_dir = workspace_root.join("packages/my-pkg");
        fs::create_dir_all(&package_dir).unwrap();

        // Create source files
        fs::create_dir_all(package_dir.join("src")).unwrap();
        fs::write(package_dir.join("src/index.ts"), "export const a = 1;").unwrap();
        fs::write(package_dir.join("src/utils.ts"), "export const b = 2;").unwrap();
        fs::write(package_dir.join("src/utils.test.ts"), "test('a', () => {});").unwrap();

        // Create nested directory
        fs::create_dir_all(package_dir.join("src/lib")).unwrap();
        fs::write(package_dir.join("src/lib/helper.ts"), "export const c = 3;").unwrap();
        fs::write(package_dir.join("src/lib/helper.test.ts"), "test('c', () => {});").unwrap();

        // Create other files
        fs::write(package_dir.join("package.json"), "{}").unwrap();
        fs::write(package_dir.join("README.md"), "# Readme").unwrap();

        let package_abs = AbsolutePathBuf::new(package_dir.into_path_buf()).unwrap();
        (temp_dir, workspace_root, package_abs)
    }

    #[test]
    fn test_empty_positive_globs_returns_empty() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive = std::collections::BTreeSet::new();
        let negative = std::collections::BTreeSet::new();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_positive_glob() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("src/**/*.ts".into()).collect();
        let negative = std::collections::BTreeSet::new();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // Should match all .ts files in src/
        assert_eq!(result.len(), 5);
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap())
        );
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.ts").unwrap())
        );
        assert!(
            result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.test.ts").unwrap())
        );
        assert!(
            result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/lib/helper.ts").unwrap())
        );
        assert!(result.contains_key(
            &RelativePathBuf::new("packages/my-pkg/src/lib/helper.test.ts").unwrap()
        ));
    }

    #[test]
    fn test_positive_with_negative_exclusion() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("src/**/*.ts".into()).collect();
        let negative: std::collections::BTreeSet<Str> =
            std::iter::once("**/*.test.ts".into()).collect();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // Should match only non-test .ts files
        assert_eq!(result.len(), 3);
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap())
        );
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.ts").unwrap())
        );
        assert!(
            result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/lib/helper.ts").unwrap())
        );
        // Test files should be excluded
        assert!(
            !result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.test.ts").unwrap())
        );
        assert!(!result.contains_key(
            &RelativePathBuf::new("packages/my-pkg/src/lib/helper.test.ts").unwrap()
        ));
    }

    #[test]
    fn test_multiple_positive_globs() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            ["src/**/*.ts".into(), "package.json".into()].into_iter().collect();
        let negative: std::collections::BTreeSet<Str> =
            std::iter::once("**/*.test.ts".into()).collect();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // Should include .ts files (excluding tests) plus package.json
        assert_eq!(result.len(), 4);
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap())
        );
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.ts").unwrap())
        );
        assert!(
            result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/lib/helper.ts").unwrap())
        );
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/package.json").unwrap())
        );
    }

    #[test]
    fn test_multiple_negative_globs() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            ["src/**/*.ts".into(), "*.md".into()].into_iter().collect();
        let negative: std::collections::BTreeSet<Str> =
            ["**/*.test.ts".into(), "**/*.md".into()].into_iter().collect();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // Should exclude both test files and markdown files
        assert_eq!(result.len(), 3);
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap())
        );
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/my-pkg/src/utils.ts").unwrap())
        );
        assert!(
            result
                .contains_key(&RelativePathBuf::new("packages/my-pkg/src/lib/helper.ts").unwrap())
        );
        assert!(!result.contains_key(&RelativePathBuf::new("packages/my-pkg/README.md").unwrap()));
    }

    #[test]
    fn test_negative_only_returns_empty() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> = std::collections::BTreeSet::new();
        let negative: std::collections::BTreeSet<Str> =
            std::iter::once("**/*.test.ts".into()).collect();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // No positive globs means empty result (negative globs alone don't select anything)
        assert!(result.is_empty());
    }

    #[test]
    fn test_file_hashes_are_consistent() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("src/index.ts".into()).collect();
        let negative = std::collections::BTreeSet::new();

        // Run twice and compare hashes
        let result1 = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();
        let result2 = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        assert_eq!(result1, result2);
    }

    #[test]
    fn test_file_hashes_change_with_content() {
        let (temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("src/index.ts".into()).collect();
        let negative = std::collections::BTreeSet::new();

        // Get initial hash
        let result1 = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();
        let hash1 =
            result1.get(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap()).unwrap();

        // Modify file content
        let file_path = temp.path().join("packages/my-pkg/src/index.ts");
        fs::write(&file_path, "export const a = 999;").unwrap();

        // Get new hash
        let result2 = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();
        let hash2 =
            result2.get(&RelativePathBuf::new("packages/my-pkg/src/index.ts").unwrap()).unwrap();

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_skips_directories() {
        let (_temp, workspace, package) = create_test_workspace();
        // This glob could match the `src/lib` directory if not filtered
        let positive: std::collections::BTreeSet<Str> = std::iter::once("src/*".into()).collect();
        let negative = std::collections::BTreeSet::new();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // Should only have files, not directories
        for path in result.keys() {
            assert!(!path.as_str().ends_with("/lib"));
            assert!(!path.as_str().ends_with("\\lib"));
        }
    }

    #[test]
    fn test_no_matching_files_returns_empty() {
        let (_temp, workspace, package) = create_test_workspace();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("nonexistent/**/*.xyz".into()).collect();
        let negative = std::collections::BTreeSet::new();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();
        assert!(result.is_empty());
    }

    /// Creates a workspace with a sibling package for testing `..` globs
    fn create_workspace_with_sibling() -> (TempDir, AbsolutePathBuf, AbsolutePathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(temp_dir.path().to_path_buf()).unwrap();

        // Create sub-pkg
        let sub_pkg = workspace_root.join("packages/sub-pkg");
        fs::create_dir_all(sub_pkg.join("src")).unwrap();
        fs::write(sub_pkg.join("src/main.ts"), "export const sub = 1;").unwrap();

        // Create sibling shared package
        let shared = workspace_root.join("packages/shared");
        fs::create_dir_all(shared.join("src")).unwrap();
        fs::create_dir_all(shared.join("dist")).unwrap();
        fs::write(shared.join("src/utils.ts"), "export const shared = 1;").unwrap();
        fs::write(shared.join("dist/output.js"), "// output").unwrap();

        let sub_pkg_abs = AbsolutePathBuf::new(sub_pkg.into_path_buf()).unwrap();
        (temp_dir, workspace_root, sub_pkg_abs)
    }

    #[test]
    fn test_dotdot_positive_glob_matches_sibling_package() {
        let (_temp, workspace, sub_pkg) = create_workspace_with_sibling();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("../shared/src/**".into()).collect();
        let negative = std::collections::BTreeSet::new();

        let result = compute_globbed_inputs(&sub_pkg, &workspace, &positive, &negative).unwrap();
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/shared/src/utils.ts").unwrap()),
            "should find sibling package file via ../shared/src/**"
        );
    }

    #[test]
    fn test_dotdot_negative_glob_excludes_from_sibling() {
        let (_temp, workspace, sub_pkg) = create_workspace_with_sibling();
        let positive: std::collections::BTreeSet<Str> =
            std::iter::once("../shared/**".into()).collect();
        let negative: std::collections::BTreeSet<Str> =
            std::iter::once("../shared/dist/**".into()).collect();

        let result = compute_globbed_inputs(&sub_pkg, &workspace, &positive, &negative).unwrap();
        assert!(
            result.contains_key(&RelativePathBuf::new("packages/shared/src/utils.ts").unwrap()),
            "should include non-excluded sibling file"
        );
        assert!(
            !result.contains_key(&RelativePathBuf::new("packages/shared/dist/output.js").unwrap()),
            "should exclude dist via ../shared/dist/**"
        );
    }

    #[test]
    fn test_overlapping_positive_globs_deduplicates() {
        let (_temp, workspace, package) = create_test_workspace();
        // Both patterns match src/index.ts
        let positive: std::collections::BTreeSet<Str> =
            ["src/**/*.ts".into(), "src/index.ts".into()].into_iter().collect();
        let negative: std::collections::BTreeSet<Str> =
            std::iter::once("**/*.test.ts".into()).collect();

        let result = compute_globbed_inputs(&package, &workspace, &positive, &negative).unwrap();

        // BTreeMap naturally deduplicates by key
        assert_eq!(result.len(), 3);
    }
}

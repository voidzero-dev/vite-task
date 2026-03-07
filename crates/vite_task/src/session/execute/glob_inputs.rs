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

use vite_glob::AnchoredGlob;
#[cfg(test)]
use vite_path::AbsolutePathBuf;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use wax::{Glob, walk::Entry as _};

/// Collect walk entries into the result map, filtering against resolved negatives.
///
/// Each positive glob is partitioned into an invariant prefix and a variant pattern.
/// The prefix is joined with `base_dir` and cleaned (normalizing `..`) to get the walk root.
/// The variant pattern is then walked from the cleaned root.
///
/// Walk errors for non-existent directories are skipped gracefully.
fn collect_walk_entries(
    walk: impl Iterator<Item = Result<wax::walk::GlobEntry, wax::walk::WalkError>>,
    workspace_root: &AbsolutePath,
    resolved_negatives: &[AnchoredGlob],
    result: &mut BTreeMap<RelativePathBuf, u64>,
) -> anyhow::Result<()> {
    use path_clean::PathClean as _;

    for entry in walk {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                // WalkError -> io::Error preserves the error kind
                let io_err: io::Error = err.into();
                if io_err.kind() == io::ErrorKind::NotFound {
                    continue;
                }
                return Err(io_err.into());
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        // Clean the path to normalize `..` components (from globs like `../shared/src/**`)
        let cleaned_path = entry.path().clean();

        // Convert to AbsolutePath for negative matching and workspace-relative stripping
        let Some(cleaned_abs) = AbsolutePath::new(&cleaned_path) else {
            continue;
        };

        // Filter against resolved negatives
        if resolved_negatives.iter().any(|neg| neg.is_match(cleaned_abs)) {
            continue;
        }

        // Compute path relative to workspace_root for the result
        let Some(relative_to_workspace) = cleaned_abs.strip_prefix(workspace_root).ok().flatten()
        else {
            continue; // Skip if path is outside workspace_root
        };

        // Hash file content
        match hash_file_content(&cleaned_path) {
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
    Ok(())
}

/// Compute globbed inputs by walking positive glob patterns and filtering with negative patterns.
///
/// Each glob is partitioned into an invariant prefix and a variant pattern. The prefix is
/// joined with `base_dir` and cleaned to normalize `..` components, producing the walk root.
/// The variant pattern walks the cleaned root. Negative patterns are resolved the same way
/// and used to filter walked entries by matching against cleaned absolute paths.
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
pub fn compute_globbed_inputs(
    base_dir: &AbsolutePath,
    workspace_root: &AbsolutePath,
    positive_globs: &std::collections::BTreeSet<Str>,
    negative_globs: &std::collections::BTreeSet<Str>,
) -> anyhow::Result<BTreeMap<RelativePathBuf, u64>> {
    use path_clean::PathClean as _;

    if positive_globs.is_empty() {
        return Ok(BTreeMap::new());
    }

    let resolved_negatives: Vec<AnchoredGlob> = negative_globs
        .iter()
        .map(|p| Ok(AnchoredGlob::new(p.as_str(), base_dir)?))
        .collect::<anyhow::Result<_>>()?;

    let mut result = BTreeMap::new();

    for pattern in positive_globs {
        let pos = Glob::new(pattern.as_str())?.into_owned();
        let (pos_prefix, pos_variant) = pos.partition();
        let walk_root = base_dir.as_path().join(&pos_prefix).clean();

        if let Some(variant_glob) = pos_variant {
            if walk_root.is_dir() {
                collect_walk_entries(
                    variant_glob.into_owned().walk(&walk_root),
                    workspace_root,
                    &resolved_negatives,
                    &mut result,
                )?;
            }
        } else {
            // Invariant-only glob (specific file path) — hash directly if it exists
            if walk_root.is_file()
                && let Some(relative) = walk_root
                    .strip_prefix(workspace_root.as_path())
                    .ok()
                    .and_then(|p| RelativePathBuf::new(p).ok())
            {
                match hash_file_content(&walk_root) {
                    Ok(hash) => {
                        result.insert(relative, hash);
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
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

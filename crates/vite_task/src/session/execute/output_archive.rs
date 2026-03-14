//! Output archive collection and restoration for cache portability.
//!
//! Collects files matching output globs into a tar archive stored on the filesystem,
//! and restores them on cache hit. All paths in the archive are workspace-root-relative.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    hash::Hasher as _,
    io,
};

use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use wax::{Glob, walk::Entry as _};

/// Collect files matching output globs into a tar archive, write it to the outputs
/// directory, and return the archive filename.
///
/// The filename is a content hash of the archive data, ensuring deduplication.
/// Returns an empty `Str` if no files match the output globs.
pub fn collect_and_store_outputs(
    workspace_root: &AbsolutePath,
    output_globs: &BTreeSet<Str>,
    outputs_dir: &AbsolutePath,
) -> anyhow::Result<Str> {
    let archive_data = collect_outputs(workspace_root, output_globs)?;
    if archive_data.is_empty() {
        return Ok(Str::default());
    }

    // Hash the archive content for the filename
    let mut hasher = twox_hash::XxHash3_64::default();
    hasher.write(&archive_data);
    let hash = hasher.finish();
    let filename = vite_str::format!("{hash:016x}.tar");

    let archive_path = outputs_dir.join(filename.as_str());
    fs::write(archive_path.as_path(), &archive_data)?;

    Ok(filename)
}

/// Collect files matching output globs into a tar archive in memory.
///
/// All paths in the archive are workspace-root-relative (forward slashes).
/// Only regular files are included (directories are implicit from paths).
///
/// Returns an empty `Vec` if no files match the output globs.
fn collect_outputs(
    workspace_root: &AbsolutePath,
    output_globs: &BTreeSet<Str>,
) -> anyhow::Result<Vec<u8>> {
    if output_globs.is_empty() {
        return Ok(Vec::new());
    }

    let mut archive_buf = Vec::new();
    let mut has_entries = false;
    {
        let mut builder = tar::Builder::new(&mut archive_buf);

        for pattern in output_globs {
            let glob = Glob::new(pattern.as_str())?.into_owned();
            let walk = glob.walk(workspace_root.as_path());
            for entry in walk {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(err) => {
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

                let path = entry.path();

                let Some(stripped) = path.strip_prefix(workspace_root.as_path()).ok() else {
                    continue;
                };
                let relative = RelativePathBuf::new(stripped)?;

                let mut file = match File::open(path) {
                    Ok(f) => f,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(err.into()),
                };

                let metadata = file.metadata()?;
                let mut header = tar::Header::new_gnu();
                header.set_size(metadata.len());
                header.set_mode(0o644);
                header.set_cksum();

                // Use the workspace-root-relative path (forward slashes) as the archive path
                builder.append_data(&mut header, relative.as_str(), &mut file)?;
                has_entries = true;
            }
        }

        builder.finish()?;
    }

    if !has_entries {
        return Ok(Vec::new());
    }

    Ok(archive_buf)
}

/// Restore output files from a tar archive.
///
/// Before extraction, removes existing files that match the output globs
/// to ensure exact state restoration (no stale files from previous runs).
///
/// All paths in the archive are workspace-root-relative.
pub fn restore_outputs(
    workspace_root: &AbsolutePath,
    output_globs: &BTreeSet<Str>,
    archive_data: &[u8],
) -> anyhow::Result<()> {
    if archive_data.is_empty() {
        return Ok(());
    }

    // Clean output files before extraction
    clean_output_paths(workspace_root, output_globs)?;

    // Extract the archive
    let mut archive = tar::Archive::new(archive_data);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?;
        let entry_path_str = entry_path.to_string_lossy();

        // Validate the path is relative and safe
        let relative = RelativePathBuf::new(entry_path_str.as_ref())?;
        let full_path = workspace_root.join(&relative);

        // Create parent directories
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write the file
        let mut file = File::create(full_path.as_path())?;
        io::copy(&mut entry, &mut file)?;
    }

    Ok(())
}

/// Remove existing files that match output globs before restoring from cache.
fn clean_output_paths(
    workspace_root: &AbsolutePath,
    output_globs: &BTreeSet<Str>,
) -> anyhow::Result<()> {
    for pattern in output_globs {
        let glob = Glob::new(pattern.as_str())?.into_owned();
        let walk = glob.walk(workspace_root.as_path());
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    let io_err: io::Error = err.into();
                    if io_err.kind() == io::ErrorKind::NotFound {
                        continue;
                    }
                    return Err(io_err.into());
                }
            };

            if entry.file_type().is_file() {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use vite_path::AbsolutePathBuf;

    use super::*;

    fn create_test_workspace() -> (TempDir, AbsolutePathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(temp_dir.path().to_path_buf()).unwrap();

        // Create package with dist output
        let pkg_dir = workspace_root.join("packages/my-pkg");
        fs::create_dir_all(pkg_dir.join("dist/nested")).unwrap();
        fs::write(pkg_dir.join("dist/index.js"), "console.log('hello');").unwrap();
        fs::write(pkg_dir.join("dist/nested/util.js"), "export const x = 1;").unwrap();
        fs::write(pkg_dir.join("dist/styles.css"), "body { color: red; }").unwrap();

        // Also create src (not an output)
        fs::create_dir_all(pkg_dir.join("src")).unwrap();
        fs::write(pkg_dir.join("src/index.ts"), "export const a = 1;").unwrap();

        (temp_dir, workspace_root)
    }

    #[test]
    fn test_collect_and_restore_outputs() {
        let (_temp, workspace) = create_test_workspace();

        let output_globs: BTreeSet<Str> =
            std::iter::once("packages/my-pkg/dist/**".into()).collect();

        // Create outputs directory
        let outputs_dir = workspace.join("cache-outputs");
        fs::create_dir_all(&outputs_dir).unwrap();

        // Collect outputs into archive file
        let archive_name =
            collect_and_store_outputs(&workspace, &output_globs, &outputs_dir).unwrap();
        assert!(!archive_name.is_empty());
        assert!(archive_name.as_str().ends_with(".tar"));

        // Remove dist directory to simulate CI fresh checkout
        fs::remove_dir_all(workspace.join("packages/my-pkg/dist")).unwrap();
        assert!(!workspace.join("packages/my-pkg/dist/index.js").as_path().exists());

        // Read archive from file and restore
        let archive_path = outputs_dir.join(archive_name.as_str());
        let archive_data = fs::read(archive_path.as_path()).unwrap();
        restore_outputs(&workspace, &output_globs, &archive_data).unwrap();

        // Verify files are restored
        assert_eq!(
            fs::read_to_string(workspace.join("packages/my-pkg/dist/index.js").as_path()).unwrap(),
            "console.log('hello');"
        );
        assert_eq!(
            fs::read_to_string(workspace.join("packages/my-pkg/dist/nested/util.js").as_path())
                .unwrap(),
            "export const x = 1;"
        );
        assert_eq!(
            fs::read_to_string(workspace.join("packages/my-pkg/dist/styles.css").as_path())
                .unwrap(),
            "body { color: red; }"
        );

        // Verify src was not affected
        assert_eq!(
            fs::read_to_string(workspace.join("packages/my-pkg/src/index.ts").as_path()).unwrap(),
            "export const a = 1;"
        );
    }

    #[test]
    fn test_empty_output_globs() {
        let (_temp, workspace) = create_test_workspace();
        let output_globs: BTreeSet<Str> = BTreeSet::new();
        let outputs_dir = workspace.join("cache-outputs");
        fs::create_dir_all(&outputs_dir).unwrap();

        let archive_name =
            collect_and_store_outputs(&workspace, &output_globs, &outputs_dir).unwrap();
        assert!(archive_name.is_empty());
    }

    #[test]
    fn test_empty_archive_restore_is_noop() {
        let (_temp, workspace) = create_test_workspace();
        let output_globs: BTreeSet<Str> =
            std::iter::once("packages/my-pkg/dist/**".into()).collect();

        // Restoring an empty archive should be a no-op
        restore_outputs(&workspace, &output_globs, &[]).unwrap();

        // Original files should still exist
        assert!(workspace.join("packages/my-pkg/dist/index.js").as_path().exists());
    }

    #[test]
    fn test_no_matching_files() {
        let (_temp, workspace) = create_test_workspace();
        let output_globs: BTreeSet<Str> =
            std::iter::once("packages/my-pkg/nonexistent/**".into()).collect();
        let outputs_dir = workspace.join("cache-outputs");
        fs::create_dir_all(&outputs_dir).unwrap();

        let archive_name =
            collect_and_store_outputs(&workspace, &output_globs, &outputs_dir).unwrap();
        // No matching files → empty archive name
        assert!(archive_name.is_empty());
    }

    #[test]
    fn test_clean_removes_stale_files() {
        let (_temp, workspace) = create_test_workspace();

        let output_globs: BTreeSet<Str> =
            std::iter::once("packages/my-pkg/dist/**".into()).collect();
        let outputs_dir = workspace.join("cache-outputs");
        fs::create_dir_all(&outputs_dir).unwrap();

        // Collect outputs (only has 3 files)
        let archive_name =
            collect_and_store_outputs(&workspace, &output_globs, &outputs_dir).unwrap();

        // Add a stale file
        fs::write(workspace.join("packages/my-pkg/dist/stale.js"), "stale").unwrap();
        assert!(workspace.join("packages/my-pkg/dist/stale.js").as_path().exists());

        // Restore should clean before extracting
        let archive_path = outputs_dir.join(archive_name.as_str());
        let archive_data = fs::read(archive_path.as_path()).unwrap();
        restore_outputs(&workspace, &output_globs, &archive_data).unwrap();

        // Stale file should be gone
        assert!(!workspace.join("packages/my-pkg/dist/stale.js").as_path().exists());

        // Original files should be restored
        assert!(workspace.join("packages/my-pkg/dist/index.js").as_path().exists());
    }
}

use std::{fs, sync::Arc};

use vite_path::{AbsolutePath, RelativePathBuf};

use crate::Error;

/// The contents of a file bundled with its absolute path for error context.
///
/// The file is read to memory on construction and its handle closed
/// immediately — the struct itself never holds a live OS file handle. This
/// keeps long-lived `WorkspaceRoot`s (held across an entire `vp run` session)
/// from pinning files like `pnpm-workspace.yaml`, which on Windows could
/// otherwise block pnpm's atomic write-and-rename and fail with EPERM
/// (<https://github.com/voidzero-dev/vite-plus/issues/1357>).
#[derive(Debug)]
pub struct FileWithPath {
    content: Vec<u8>,
    path: Arc<AbsolutePath>,
}

impl FileWithPath {
    /// Open a file at the given path and read its contents into memory.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read.
    pub fn open(path: Arc<AbsolutePath>) -> Result<Self, Error> {
        let content = fs::read(&*path)?;
        Ok(Self { content, path })
    }

    /// Try to read a file, returning None if it doesn't exist.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read.
    pub fn open_if_exists(path: Arc<AbsolutePath>) -> Result<Option<Self>, Error> {
        match fs::read(&*path) {
            Ok(content) => Ok(Some(Self { content, path })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get the file contents as a byte slice.
    #[must_use]
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Get the file path.
    #[must_use]
    pub const fn path(&self) -> &Arc<AbsolutePath> {
        &self.path
    }
}

/// The package root directory and its package.json file.
#[derive(Debug)]
pub struct PackageRoot<'a> {
    pub path: &'a AbsolutePath,
    pub cwd: RelativePathBuf,
    pub package_json: FileWithPath,
}

/// Find the package root directory from the current working directory. `original_cwd` must be absolute.
///
/// If the package.json file is not found, will return `PackageJsonNotFound` error.
///
/// # Errors
/// Returns an error if no `package.json` is found in any ancestor directory, or if a path cannot be stripped.
///
/// # Panics
/// Panics if `original_cwd` is not within the found package root (should not happen in practice).
pub fn find_package_root(original_cwd: &AbsolutePath) -> Result<PackageRoot<'_>, Error> {
    let mut cwd = original_cwd;
    loop {
        // Check for package.json
        let package_json_path: Arc<AbsolutePath> = cwd.join("package.json").into();
        if let Some(file_with_path) = FileWithPath::open_if_exists(package_json_path)? {
            return Ok(PackageRoot {
                path: cwd,
                cwd: original_cwd.strip_prefix(cwd)?.expect("cwd must be within the package root"),
                package_json: file_with_path,
            });
        }

        if let Some(parent) = cwd.parent() {
            // Move up one directory
            cwd = parent;
        } else {
            // We've reached the root, return PackageJsonNotFound error.
            return Err(Error::PackageJsonNotFound(original_cwd.to_absolute_path_buf()));
        }
    }
}

/// The workspace file.
///
/// - `PnpmWorkspaceYaml` is the pnpm workspace file.
/// - `NpmWorkspaceJson` is the package.json file of a yarn/npm workspace.
/// - `NonWorkspacePackage` is the package.json file of a non-workspace package.
#[derive(Debug)]
pub enum WorkspaceFile {
    /// The pnpm-workspace.yaml file of a pnpm workspace.
    PnpmWorkspaceYaml(FileWithPath),
    /// The package.json file of a yarn/npm workspace.
    NpmWorkspaceJson(FileWithPath),
    /// The package.json file of a non-workspace package.
    NonWorkspacePackage(FileWithPath),
}

/// The workspace root directory and its workspace file.
///
/// If the workspace file is not found, but a package is found, `workspace_file` will be `NonWorkspacePackage` with the `package.json` File.
#[derive(Debug)]
pub struct WorkspaceRoot {
    /// The absolute path of the workspace root directory.
    pub path: Arc<AbsolutePath>,
    /// The workspace file.
    pub workspace_file: WorkspaceFile,
}

/// Find the workspace root directory from the current working directory. `original_cwd` must be absolute.
///
/// Returns the workspace root and the relative path from the workspace root to the original cwd.
///
/// If the workspace file is not found, but a package is found, `workspace_file` will be `NonWorkspacePackage` with the `package.json` File.
///
/// If neither workspace nor package is found, will return `PackageJsonNotFound` error.
///
/// # Errors
/// Returns an error if no workspace or package is found, or if file I/O or JSON/YAML parsing fails.
///
/// # Panics
/// Panics if `original_cwd` is not within the found workspace root (should not happen in practice).
pub fn find_workspace_root(
    original_cwd: &AbsolutePath,
) -> Result<(WorkspaceRoot, RelativePathBuf), Error> {
    let mut cwd = original_cwd;

    loop {
        // Check for pnpm-workspace.yaml for pnpm workspace
        let pnpm_workspace_path: Arc<AbsolutePath> = cwd.join("pnpm-workspace.yaml").into();
        if let Some(file_with_path) = FileWithPath::open_if_exists(pnpm_workspace_path)? {
            let relative_cwd =
                original_cwd.strip_prefix(cwd)?.expect("cwd must be within the pnpm workspace");
            return Ok((
                WorkspaceRoot {
                    path: Arc::from(cwd),
                    workspace_file: WorkspaceFile::PnpmWorkspaceYaml(file_with_path),
                },
                relative_cwd,
            ));
        }

        // Check for package.json with workspaces field for npm/yarn workspace
        let package_json_path: Arc<AbsolutePath> = cwd.join("package.json").into();
        if let Some(file_with_path) = FileWithPath::open_if_exists(package_json_path)? {
            let package_json: serde_json::Value = serde_json::from_slice(file_with_path.content())
                .map_err(|e| Error::SerdeJson {
                    file_path: Arc::clone(file_with_path.path()),
                    serde_json_error: e,
                })?;
            if package_json.get("workspaces").is_some() {
                let relative_cwd =
                    original_cwd.strip_prefix(cwd)?.expect("cwd must be within the workspace");
                return Ok((
                    WorkspaceRoot {
                        path: Arc::from(cwd),
                        workspace_file: WorkspaceFile::NpmWorkspaceJson(file_with_path),
                    },
                    relative_cwd,
                ));
            }
        }

        // TODO(@fengmk2): other package manager support

        // Move up one directory
        if let Some(parent) = cwd.parent() {
            cwd = parent;
        } else {
            // We've reached the root, try to find the package root and return the non-workspace package.
            let package_root = find_package_root(original_cwd)?;
            let workspace_file = WorkspaceFile::NonWorkspacePackage(package_root.package_json);
            return Ok((
                WorkspaceRoot { path: Arc::from(package_root.path), workspace_file },
                package_root.cwd,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    /// Regression test for <https://github.com/voidzero-dev/vite-plus/issues/1357>:
    /// on Windows, an open handle to `pnpm-workspace.yaml` without
    /// `FILE_SHARE_DELETE` blocks pnpm's atomic write-tmp-then-rename.
    #[test]
    fn find_workspace_root_does_not_lock_pnpm_workspace_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = AbsolutePath::new(temp_dir.path()).unwrap();
        let ws_yaml = temp_dir_path.join("pnpm-workspace.yaml");
        let ws_yaml_tmp = temp_dir_path.join("pnpm-workspace.yaml.tmp");

        fs::write(&ws_yaml, b"packages:\n  - apps/*\n").unwrap();

        let (workspace_root, _) = find_workspace_root(temp_dir_path).unwrap();

        fs::write(&ws_yaml_tmp, b"packages:\n  - apps/*\n  - packages/*\n").unwrap();
        fs::rename(&ws_yaml_tmp, &ws_yaml)
            .expect("rename over pnpm-workspace.yaml must succeed while WorkspaceRoot is alive");

        drop(workspace_root);
    }

    /// Linux-only: `/proc/self/fd` lets us verify no descriptor remains
    /// pointing at `pnpm-workspace.yaml` regardless of Rust's default
    /// share mode on the platform.
    #[cfg(target_os = "linux")]
    #[test]
    fn find_workspace_root_releases_pnpm_workspace_yaml_fd_on_linux() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = AbsolutePath::new(temp_dir.path()).unwrap();
        let ws_yaml = temp_dir_path.join("pnpm-workspace.yaml");
        fs::write(&ws_yaml, b"packages:\n  - apps/*\n").unwrap();

        let (workspace_root, _) = find_workspace_root(temp_dir_path).unwrap();

        let ws_yaml_canonical = fs::canonicalize(&ws_yaml).unwrap();
        let mut open_to_target = 0;
        for entry in fs::read_dir("/proc/self/fd").unwrap().flatten() {
            if let Ok(link) = fs::read_link(entry.path())
                && link == ws_yaml_canonical
            {
                open_to_target += 1;
            }
        }
        assert_eq!(
            open_to_target, 0,
            "expected no open file descriptor for pnpm-workspace.yaml, got {open_to_target}",
        );
        drop(workspace_root);
    }

    #[test]
    fn file_with_path_content_matches_file_on_disk() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = AbsolutePath::new(temp_dir.path()).unwrap();
        let path: Arc<AbsolutePath> = temp_dir_path.join("pnpm-workspace.yaml").into();
        fs::write(&*path, b"packages:\n  - apps/*\n").unwrap();

        let file_with_path = FileWithPath::open(Arc::clone(&path)).unwrap();
        assert_eq!(file_with_path.content(), b"packages:\n  - apps/*\n");
        assert_eq!(&**file_with_path.path(), &*path);
    }

    #[test]
    fn file_with_path_open_if_exists_returns_none_when_missing() {
        let temp_dir = TempDir::new().unwrap();
        let temp_dir_path = AbsolutePath::new(temp_dir.path()).unwrap();
        let path: Arc<AbsolutePath> = temp_dir_path.join("missing.yaml").into();
        assert!(FileWithPath::open_if_exists(path).unwrap().is_none());
    }
}

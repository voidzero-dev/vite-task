//! Post-run fingerprinting for execution caching.
//!
//! This module provides types and functions for creating and validating
//! fingerprints of file system state after task execution.

use std::{
    fs::File,
    hash::Hasher as _,
    io::{self, BufRead, Read},
    sync::Arc,
};

use bincode::{Decode, Encode};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use vite_glob::GlobPatternSet;
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;

use super::spawn::PathRead;
use crate::collections::HashMap;

/// Post-run fingerprint capturing file state after execution.
/// Used to validate whether cached outputs are still valid.
#[derive(Encode, Decode, Debug, Serialize)]
pub struct PostRunFingerprint {
    /// Paths accessed during execution with their content fingerprints
    pub inputs: HashMap<RelativePathBuf, PathFingerprint>,
}

/// Fingerprint for a single path (file or directory)
#[derive(Encode, Decode, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum PathFingerprint {
    /// Path was not found when fingerprinting
    NotFound,
    /// File content hash using `xxHash3_64`
    FileContentHash(u64),
    /// Directory with optional entry listing.
    /// `Folder(None)` means the directory was opened but entries were not read
    /// (e.g., for `openat` calls).
    /// `Folder(Some(_))` contains the directory entries.
    Folder(Option<HashMap<Str, DirEntryKind>>),
}

/// Kind of directory entry
#[derive(Encode, Decode, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum DirEntryKind {
    File,
    Dir,
    Symlink,
}

/// Describes why the post-run fingerprint validation failed
#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PostRunFingerprintMismatch {
    InputContentChanged { path: RelativePathBuf },
}

impl std::fmt::Display for PostRunFingerprintMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputContentChanged { path } => {
                write!(f, "{path} content changed")
            }
        }
    }
}

impl PostRunFingerprint {
    /// Creates a new fingerprint from path accesses after task execution.
    ///
    /// # Arguments
    /// * `path_reads` - Map of paths that were read during execution
    /// * `base_dir` - Workspace root for resolving relative paths
    /// * `fingerprint_ignores` - Optional glob patterns to exclude from fingerprinting
    pub fn create(
        path_reads: &HashMap<RelativePathBuf, PathRead>,
        base_dir: &AbsolutePath,
        fingerprint_ignores: Option<&[Str]>,
    ) -> anyhow::Result<Self> {
        // Build ignore matcher from patterns if provided
        let ignore_matcher = fingerprint_ignores
            .filter(|patterns| !patterns.is_empty())
            .map(GlobPatternSet::new)
            .transpose()?;

        let inputs = path_reads
            .par_iter()
            .filter(|(path, _)| {
                // Apply ignore patterns if present
                ignore_matcher.as_ref().is_none_or(|matcher| !matcher.is_match(path.as_str()))
            })
            .map(|(relative_path, path_read)| {
                let full_path = Arc::<AbsolutePath>::from(base_dir.join(relative_path));
                let fingerprint = fingerprint_path(&full_path, *path_read)?;
                Ok((relative_path.clone(), fingerprint))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        Ok(Self { inputs })
    }

    /// Validates the fingerprint against current filesystem state.
    /// Returns `Some(mismatch)` if validation fails, `None` if valid.
    pub fn validate(
        &self,
        base_dir: &AbsolutePath,
    ) -> anyhow::Result<Option<PostRunFingerprintMismatch>> {
        let input_mismatch =
            self.inputs.par_iter().find_map_any(|(input_relative_path, path_fingerprint)| {
                let input_full_path = Arc::<AbsolutePath>::from(base_dir.join(input_relative_path));
                let path_read = PathRead {
                    read_dir_entries: matches!(path_fingerprint, PathFingerprint::Folder(Some(_))),
                };
                let current_path_fingerprint = match fingerprint_path(&input_full_path, path_read) {
                    Ok(ok) => ok,
                    Err(err) => return Some(Err(err)),
                };
                if path_fingerprint == &current_path_fingerprint {
                    None
                } else {
                    Some(Ok(PostRunFingerprintMismatch::InputContentChanged {
                        path: input_relative_path.clone(),
                    }))
                }
            });
        input_mismatch.transpose()
    }
}

/// Hash file content using `xxHash3_64`
fn hash_content(mut stream: impl Read) -> io::Result<u64> {
    let mut hasher = twox_hash::XxHash3_64::default();
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// Check if a directory entry should be ignored in fingerprinting
fn should_ignore_entry(name: &[u8]) -> bool {
    matches!(name, b"." | b".." | b".DS_Store") || name.eq_ignore_ascii_case(b"dist")
}

/// Fingerprint a single path
pub fn fingerprint_path(
    path: &Arc<AbsolutePath>,
    path_read: PathRead,
) -> anyhow::Result<PathFingerprint> {
    let std_path = path.as_path();

    let file = match File::open(std_path) {
        Ok(file) => file,
        Err(err) => {
            // On Windows, File::open fails specifically for directories with PermissionDenied
            #[cfg(windows)]
            {
                if err.kind() == io::ErrorKind::PermissionDenied {
                    // This might be a directory - try reading it as such
                    return process_directory(std_path, path_read);
                }
                // On Windows, paths with trailing backslash (from joining empty path)
                // fail with NotFound (error code 3). Try as directory in this case.
                if err.raw_os_error() == Some(3) && std_path.to_string_lossy().ends_with('\\') {
                    return process_directory(std_path, path_read);
                }
            }
            if err.kind() != io::ErrorKind::NotFound {
                tracing::trace!(
                    "Uncommon error when opening {:?} for fingerprinting: {}",
                    std_path,
                    err
                );
            }
            // Treat all open errors as NotFound for fingerprinting purposes
            return Ok(PathFingerprint::NotFound);
        }
    };

    let mut reader = io::BufReader::new(file);
    if let Err(io_err) = reader.fill_buf() {
        if io_err.kind() != io::ErrorKind::IsADirectory {
            return Err(io_err.into());
        }
        // Is a directory on Unix - use the optimized nix implementation
        #[cfg(unix)]
        {
            return process_directory_unix(reader.get_ref(), path_read);
        }
        #[cfg(windows)]
        {
            return process_directory(std_path, path_read);
        }
    }
    Ok(PathFingerprint::FileContentHash(hash_content(reader)?))
}

/// Process a directory on Windows using std::fs::read_dir
#[cfg(windows)]
fn process_directory(
    path: &std::path::Path,
    path_read: PathRead,
) -> anyhow::Result<PathFingerprint> {
    if !path_read.read_dir_entries {
        return Ok(PathFingerprint::Folder(None));
    }

    let mut entries = HashMap::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_bytes = name.as_encoded_bytes();

        if should_ignore_entry(name_bytes) {
            continue;
        }

        let file_type = entry.file_type()?;
        let kind = if file_type.is_file() {
            DirEntryKind::File
        } else if file_type.is_dir() {
            DirEntryKind::Dir
        } else {
            DirEntryKind::Symlink
        };

        let name_str = name.to_string_lossy();
        entries.insert(Str::from(name_str.as_ref()), kind);
    }

    Ok(PathFingerprint::Folder(Some(entries)))
}

/// Process a directory on Unix using nix for efficiency
#[cfg(unix)]
fn process_directory_unix(file: &File, path_read: PathRead) -> anyhow::Result<PathFingerprint> {
    use std::os::fd::AsFd;

    if !path_read.read_dir_entries {
        return Ok(PathFingerprint::Folder(None));
    }

    let fd = file.as_fd();
    let mut dir = nix::dir::Dir::from_fd(fd.try_clone_to_owned()?)?;

    let mut entries = HashMap::new();
    for entry in dir.iter() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();

        if should_ignore_entry(name) {
            continue;
        }

        let kind = match entry.file_type() {
            Some(nix::dir::Type::Directory) => DirEntryKind::Dir,
            Some(nix::dir::Type::Symlink) => DirEntryKind::Symlink,
            // Treat files and other types as files for fingerprinting
            _ => DirEntryKind::File,
        };

        #[expect(
            clippy::disallowed_types,
            reason = "from_utf8_lossy returns Cow referencing String"
        )]
        let name_str = String::from_utf8_lossy(name);
        entries.insert(Str::from(name_str.as_ref()), kind);
    }

    Ok(PathFingerprint::Folder(Some(entries)))
}

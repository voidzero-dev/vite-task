//! Per-path fingerprints and the run's input-fingerprint record.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufRead},
    sync::Arc,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_str::Str;
use wincode::{SchemaRead, SchemaWrite};

use crate::collections::HashMap;

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Fingerprint for a single path (file or directory)
#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum PathFingerprint {
    /// Path was not found when fingerprinting
    NotFound,
    /// File content hash using `xxHash3_64`
    FileContentHash(u64),
    /// Directory with optional entry listing.
    /// `Folder(None)` means the directory was opened but entries were not read
    /// (e.g., for `openat` calls).
    /// `Folder(Some(_))` contains the directory entries sorted by name.
    Folder(Option<BTreeMap<Str, DirEntryKind>>),
}

/// Kind of directory entry
#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub enum DirEntryKind {
    File,
    Dir,
    Symlink,
}

/// How an input changed since a previous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputChangeKind {
    /// File content changed but path is the same
    ContentModified,
    /// New file or folder added
    Added,
    /// Existing file or folder removed
    Removed,
}

/// The first input found to differ from a previous run.
#[derive(Debug, Clone)]
pub struct InputChange {
    pub kind: InputChangeKind,
    pub path: RelativePathBuf,
}

/// Fingerprints of everything a task run read.
///
/// Opaque: produced at the end of a run, stored by the caller, and handed
/// back at the start of a later run to detect input changes.
#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, Debug, Default, Serialize)]
pub struct InputFingerprints {
    /// Content hashes of the explicitly-listed inputs, captured before the
    /// run (the pre-run snapshot). The read-write-overlap check guarantees
    /// the task did not modify them, so they double as the post-run state.
    snapshot: BTreeMap<RelativePathBuf, u64>,
    /// Fingerprints of the inputs the run discovered while it ran (traced
    /// reads not already covered by the snapshot).
    discovered: HashMap<RelativePathBuf, PathFingerprint>,
}

impl InputFingerprints {
    pub(crate) const fn new(
        snapshot: BTreeMap<RelativePathBuf, u64>,
        discovered: HashMap<RelativePathBuf, PathFingerprint>,
    ) -> Self {
        Self { snapshot, discovered }
    }

    /// Find the first input that differs between this (stored) record and the
    /// present: the stored snapshot is diffed against `current_snapshot`
    /// (pure), then each discovered input is re-fingerprinted against the
    /// filesystem. Check order determines which change gets reported when
    /// several exist.
    pub(crate) fn find_change(
        &self,
        current_snapshot: &BTreeMap<RelativePathBuf, u64>,
        base_dir: &AbsolutePath,
    ) -> anyhow::Result<Option<InputChange>> {
        if let Some(change) = detect_snapshot_change(&self.snapshot, current_snapshot) {
            return Ok(Some(change));
        }
        validate_discovered(&self.discovered, base_dir)
    }
}

/// Fingerprint the inputs a run discovered: every traced read not already in
/// the snapshot. Paths in the snapshot are skipped — they are already tracked
/// by the pre-run hashes, and the read-write overlap check guarantees the task
/// did not modify them, so the pre-run hash is still correct.
pub fn fingerprint_discovered(
    path_reads: &HashMap<RelativePathBuf, PathRead>,
    base_dir: &AbsolutePath,
    snapshot: &BTreeMap<RelativePathBuf, u64>,
) -> anyhow::Result<HashMap<RelativePathBuf, PathFingerprint>> {
    path_reads
        .par_iter()
        .filter(|(path, _)| !snapshot.contains_key(*path))
        .map(|(relative_path, path_read)| {
            let full_path = Arc::<AbsolutePath>::from(base_dir.join(relative_path));
            let fingerprint = fingerprint_path(&full_path, *path_read)?;
            Ok((relative_path.clone(), fingerprint))
        })
        .collect::<anyhow::Result<HashMap<_, _>>>()
}

/// Re-fingerprint each stored discovered input against the current filesystem
/// state, returning the first mismatch found (parallel, so the pick among
/// several concurrent mismatches is nondeterministic — intentional).
fn validate_discovered(
    discovered: &HashMap<RelativePathBuf, PathFingerprint>,
    base_dir: &AbsolutePath,
) -> anyhow::Result<Option<InputChange>> {
    let change = discovered.par_iter().find_map_any(|(input_relative_path, path_fingerprint)| {
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
            let (kind, entry_name) =
                determine_change_kind(path_fingerprint, &current_path_fingerprint);
            let path = if let Some(name) = entry_name {
                // For folder changes, build `dir/entry` path
                let entry = match RelativePathBuf::new(name.as_str()) {
                    Ok(p) => p,
                    Err(e) => return Some(Err(e.into())),
                };
                input_relative_path.as_relative_path().join(entry)
            } else {
                input_relative_path.clone()
            };
            Some(Ok(InputChange { kind, path }))
        }
    });
    change.transpose()
}

/// Compare stored and current snapshot hashes, returning the first changed path.
/// Both maps are `BTreeMap` so we iterate them in sorted lockstep.
fn detect_snapshot_change(
    stored: &BTreeMap<RelativePathBuf, u64>,
    current: &BTreeMap<RelativePathBuf, u64>,
) -> Option<InputChange> {
    let mut stored_iter = stored.iter();
    let mut current_iter = current.iter();
    let mut s = stored_iter.next();
    let mut c = current_iter.next();

    loop {
        match (s, c) {
            (None, None) => return None,
            (Some((sp, _)), None) => {
                return Some(InputChange { kind: InputChangeKind::Removed, path: sp.clone() });
            }
            (None, Some((cp, _))) => {
                return Some(InputChange { kind: InputChangeKind::Added, path: cp.clone() });
            }
            (Some((sp, sh)), Some((cp, ch))) => match sp.cmp(cp) {
                std::cmp::Ordering::Equal => {
                    if sh != ch {
                        return Some(InputChange {
                            kind: InputChangeKind::ContentModified,
                            path: sp.clone(),
                        });
                    }
                    s = stored_iter.next();
                    c = current_iter.next();
                }
                std::cmp::Ordering::Less => {
                    return Some(InputChange { kind: InputChangeKind::Removed, path: sp.clone() });
                }
                std::cmp::Ordering::Greater => {
                    return Some(InputChange { kind: InputChangeKind::Added, path: cp.clone() });
                }
            },
        }
    }
}

/// Determine the kind of change between two differing path fingerprints.
/// Caller guarantees `stored != current`.
///
/// Returns `(kind, entry_name)` where `entry_name` is `Some` for folder changes
/// when a specific added/removed entry can be identified.
fn determine_change_kind<'a>(
    stored: &'a PathFingerprint,
    current: &'a PathFingerprint,
) -> (InputChangeKind, Option<&'a Str>) {
    match (stored, current) {
        (PathFingerprint::NotFound, _) => (InputChangeKind::Added, None),
        (_, PathFingerprint::NotFound) => (InputChangeKind::Removed, None),
        (PathFingerprint::FileContentHash(_), PathFingerprint::FileContentHash(_)) => {
            (InputChangeKind::ContentModified, None)
        }
        (PathFingerprint::Folder(old), PathFingerprint::Folder(new)) => {
            determine_folder_change_kind(old.as_ref(), new.as_ref())
        }
        // Type changed (file ↔ folder)
        _ => (InputChangeKind::Added, None),
    }
}

/// Determine whether a folder change is an addition or removal by comparing entries.
/// Both maps are `BTreeMap` so we iterate them in sorted lockstep.
/// Returns the specific entry name that was added or removed, if identifiable.
fn determine_folder_change_kind<'a>(
    old: Option<&'a BTreeMap<Str, DirEntryKind>>,
    new: Option<&'a BTreeMap<Str, DirEntryKind>>,
) -> (InputChangeKind, Option<&'a Str>) {
    let (Some(old_entries), Some(new_entries)) = (old, new) else {
        return (InputChangeKind::Added, None);
    };

    let mut old_iter = old_entries.iter();
    let mut new_iter = new_entries.iter();
    let mut o = old_iter.next();
    let mut n = new_iter.next();

    loop {
        match (o, n) {
            (None, None) => return (InputChangeKind::Added, None),
            (Some((name, _)), None) => return (InputChangeKind::Removed, Some(name)),
            (None, Some((name, _))) => return (InputChangeKind::Added, Some(name)),
            (Some((ok, ov)), Some((nk, nv))) => match ok.cmp(nk) {
                std::cmp::Ordering::Equal => {
                    if ov != nv {
                        return (InputChangeKind::Added, Some(ok));
                    }
                    o = old_iter.next();
                    n = new_iter.next();
                }
                std::cmp::Ordering::Less => return (InputChangeKind::Removed, Some(ok)),
                std::cmp::Ordering::Greater => return (InputChangeKind::Added, Some(nk)),
            },
        }
    }
}

/// Check if a directory entry should be ignored in fingerprinting
const fn should_ignore_entry(name: &[u8]) -> bool {
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
                    "Uncommon error when opening {} for fingerprinting: {}",
                    std_path.display(),
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
    Ok(PathFingerprint::FileContentHash(super::hash::hash_content(reader)?))
}

/// Process a directory on Windows using `std::fs::read_dir`
#[cfg(windows)]
#[expect(clippy::disallowed_types, reason = "Windows fallback uses std::path::Path directly")]
fn process_directory(
    path: &std::path::Path,
    path_read: PathRead,
) -> anyhow::Result<PathFingerprint> {
    if !path_read.read_dir_entries {
        return Ok(PathFingerprint::Folder(None));
    }

    let mut entries = BTreeMap::new();
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

    let mut entries = BTreeMap::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The opaque record must survive the cache's serialization round trip
    /// with every fingerprint shape it can carry.
    #[test]
    fn input_fingerprints_wincode_round_trip() {
        let mut snapshot = BTreeMap::new();
        snapshot.insert(RelativePathBuf::new("src/a.txt").unwrap(), 42u64);

        let mut entries = BTreeMap::new();
        entries.insert(Str::from("child.txt"), DirEntryKind::File);
        entries.insert(Str::from("nested"), DirEntryKind::Dir);
        entries.insert(Str::from("link"), DirEntryKind::Symlink);

        let mut discovered = HashMap::default();
        discovered
            .insert(RelativePathBuf::new("read.txt").unwrap(), PathFingerprint::FileContentHash(7));
        discovered.insert(RelativePathBuf::new("probed").unwrap(), PathFingerprint::NotFound);
        discovered
            .insert(RelativePathBuf::new("opened-dir").unwrap(), PathFingerprint::Folder(None));
        discovered.insert(
            RelativePathBuf::new("listed-dir").unwrap(),
            PathFingerprint::Folder(Some(entries)),
        );

        let fingerprints = InputFingerprints::new(snapshot, discovered);

        let config = wincode::config::Configuration::default();
        let bytes = wincode::config::serialize(&fingerprints, config).unwrap();
        let back: InputFingerprints = wincode::config::deserialize_exact(&bytes, config).unwrap();
        assert_eq!(fingerprints, back);

        let empty = InputFingerprints::default();
        let bytes = wincode::config::serialize(&empty, config).unwrap();
        let back: InputFingerprints = wincode::config::deserialize_exact(&bytes, config).unwrap();
        assert_eq!(empty, back);
    }
}

//! Post-run fingerprinting for execution caching.
//!
//! This module provides types and functions for creating and validating
//! fingerprints of file system state after task execution.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::File,
    io::{self, BufRead},
    sync::Arc,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use vite_path::{AbsolutePath, RelativePathBuf};
use vite_str::Str;
use vite_task_plan::cache_metadata::EnvValueHash;
use wincode::{SchemaRead, SchemaWrite};

use crate::{
    collections::HashMap,
    session::cache::{EnvMismatch, InputChangeKind},
};

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Post-run fingerprint capturing file state after execution.
/// Used to validate whether cached outputs are still valid.
#[derive(SchemaWrite, SchemaRead, Debug, Default, Serialize)]
pub struct PostRunFingerprint {
    /// Paths inferred from fspy during execution with their content fingerprints.
    /// Only populated when `input_config.includes_auto` is true.
    pub inferred_inputs: HashMap<RelativePathBuf, PathFingerprint>,

    /// Env vars observed via runner-aware IPC `getEnv` with `tracked: true`.
    /// Key is the env name; value is the env value hash at execution time, or
    /// `None` if unset. Validated at cache lookup against the same plan env
    /// context that served the original request.
    pub tracked_envs: BTreeMap<Str, Option<EnvValueHash>>,

    /// Glob-pattern env queries (`getEnvs`) made with `tracked: true`.
    /// Outer key is the glob pattern, inner map is the match-set at execution
    /// time (name -> value hash). Validated at cache lookup by re-matching
    /// against the current env context and comparing the resulting set.
    ///
    /// Non-UTF-8 env names are never matched, saved, or treated as errors:
    /// they are not returned to the client, so their existence cannot affect
    /// task behavior. Values are stricter. A matched env must have a UTF-8
    /// value; the JS client errors when querying a matched non-UTF-8 value,
    /// and cache-hit validation treats a currently matched non-UTF-8 value as
    /// a changed mismatch so stale cached output is not replayed.
    pub tracked_env_globs: BTreeMap<Str, BTreeMap<Str, EnvValueHash>>,
}

/// A mismatch between the stored post-run fingerprint and the current state.
#[derive(Debug, Clone)]
pub enum PostRunMismatch {
    /// An inferred input file or directory changed.
    Input { kind: InputChangeKind, path: RelativePathBuf },
    /// A tool-tracked env var changed value, appeared, or disappeared.
    TrackedEnv(EnvMismatch),
    /// A tool-tracked env glob's match-set changed between runs. Carries the
    /// first differing entry in env-name order.
    TrackedEnvGlob { pattern: Str, mismatch: EnvMismatch },
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

impl PostRunFingerprint {
    /// Creates a new fingerprint from path accesses after task execution.
    ///
    /// Negative glob filtering is done upstream (see
    /// [`super::tracked_accesses::TrackedPathAccesses::from_raw`]).
    /// Paths already present in `globbed_inputs` are skipped — they are
    /// already tracked by the prerun glob fingerprint, and the read-write
    /// overlap check in `execute_spawn` guarantees the task did not modify
    /// them, so the prerun hash is still correct.
    ///
    /// # Arguments
    /// * `inferred_path_reads` - Map of paths that were read during execution (from fspy)
    /// * `base_dir` - Workspace root for resolving relative paths
    /// * `globbed_inputs` - Prerun glob fingerprint; paths here are skipped
    /// * `tracked_envs` - Tool-requested env vars (name -> value hash), validated on lookup
    /// * `tracked_env_globs` - Tool-requested env globs (pattern -> match-set hashes)
    #[tracing::instrument(level = "debug", skip_all, name = "create_post_run_fingerprint")]
    pub fn create(
        inferred_path_reads: &HashMap<RelativePathBuf, PathRead>,
        base_dir: &AbsolutePath,
        globbed_inputs: &BTreeMap<RelativePathBuf, u64>,
        tracked_envs: BTreeMap<Str, Option<EnvValueHash>>,
        tracked_env_globs: BTreeMap<Str, BTreeMap<Str, EnvValueHash>>,
    ) -> anyhow::Result<Self> {
        let inferred_inputs = inferred_path_reads
            .par_iter()
            .filter(|(path, _)| !globbed_inputs.contains_key(*path))
            .map(|(relative_path, path_read)| {
                let full_path = Arc::<AbsolutePath>::from(base_dir.join(relative_path));
                let fingerprint = fingerprint_path(&full_path, *path_read)?;
                Ok((relative_path.clone(), fingerprint))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        Ok(Self { inferred_inputs, tracked_envs, tracked_env_globs })
    }

    /// Validates the fingerprint against current filesystem state and the
    /// unfiltered env context used by runner-aware IPC. `unfiltered_envs` must
    /// be the same plan env context that served the original `getEnv` request,
    /// not the filtered env passed to the spawned process.
    ///
    /// Returns `Some(mismatch)` if anything changed, `None` if all valid.
    /// Returns an error if a tracked env is currently present but cannot be
    /// represented as UTF-8; treating that value as unset would make cache
    /// validation unsound.
    #[tracing::instrument(level = "debug", skip_all, name = "validate_post_run_fingerprint")]
    pub fn validate(
        &self,
        base_dir: &AbsolutePath,
        unfiltered_envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    ) -> anyhow::Result<Option<PostRunMismatch>> {
        let input_mismatch = self.inferred_inputs.par_iter().find_map_any(
            |(input_relative_path, path_fingerprint)| {
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
                    Some(Ok(PostRunMismatch::Input { kind, path }))
                }
            },
        );
        if let Some(result) = input_mismatch {
            return result.map(Some);
        }

        for (name, stored_value) in &self.tracked_envs {
            let current_value = unfiltered_envs
                .get(OsStr::new(name.as_str()))
                .map(|value| {
                    let value_str = value.to_str().ok_or_else(|| {
                        anyhow::anyhow!("tracked env value for {name} is not valid UTF-8")
                    })?;
                    Ok::<_, anyhow::Error>(EnvValueHash::new(value_str))
                })
                .transpose()?;
            if let Some(mismatch) =
                EnvMismatch::compare(name, stored_value.as_ref(), current_value.as_ref())
            {
                return Ok(Some(PostRunMismatch::TrackedEnv(mismatch)));
            }
        }

        for (pattern, stored_matches) in &self.tracked_env_globs {
            let current_matches = match match_env_glob(pattern.as_str(), unfiltered_envs)? {
                EnvGlobValidation::Matches(matches) => matches,
                EnvGlobValidation::NonUtf8Value(mismatch) => {
                    return Ok(Some(PostRunMismatch::TrackedEnvGlob {
                        pattern: pattern.clone(),
                        mismatch,
                    }));
                }
            };
            if let Some(mismatch) = first_env_glob_mismatch(stored_matches, &current_matches) {
                return Ok(Some(PostRunMismatch::TrackedEnvGlob {
                    pattern: pattern.clone(),
                    mismatch,
                }));
            }
        }

        Ok(None)
    }
}

/// Build the current match-set for `pattern` by enumerating the given env
/// snapshot and keeping UTF-8 names whose representation matches the glob. If
/// a matching env has a non-UTF-8 value, return a changed mismatch so the stale
/// cache entry is not replayed.
fn match_env_glob(
    pattern: &str,
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
) -> anyhow::Result<EnvGlobValidation> {
    let glob = vite_glob::env::EnvGlob::new(pattern)?;
    let mut matches = BTreeMap::new();
    for (name, value) in envs {
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !glob.is_match(name_str) {
            continue;
        }
        let Some(value_str) = value.to_str() else {
            return Ok(EnvGlobValidation::NonUtf8Value(EnvMismatch::Changed {
                name: Str::from(name_str),
            }));
        };
        matches.insert(Str::from(name_str), EnvValueHash::new(value_str));
    }
    Ok(EnvGlobValidation::Matches(matches))
}

enum EnvGlobValidation {
    Matches(BTreeMap<Str, EnvValueHash>),
    NonUtf8Value(EnvMismatch),
}

/// Find the first deterministic difference between stored and current env
/// glob match-sets.
fn first_env_glob_mismatch(
    stored: &BTreeMap<Str, EnvValueHash>,
    current: &BTreeMap<Str, EnvValueHash>,
) -> Option<EnvMismatch> {
    let mut stored_iter = stored.iter();
    let mut current_iter = current.iter();
    let mut s = stored_iter.next();
    let mut c = current_iter.next();

    loop {
        match (s, c) {
            (None, None) => return None,
            (Some((name, _)), None) => return Some(EnvMismatch::Removed { name: name.clone() }),
            (None, Some((name, _))) => return Some(EnvMismatch::Added { name: name.clone() }),
            (Some((sn, sv)), Some((cn, cv))) => match sn.cmp(cn) {
                std::cmp::Ordering::Equal => {
                    if sv != cv {
                        return Some(EnvMismatch::Changed { name: sn.clone() });
                    }
                    s = stored_iter.next();
                    c = current_iter.next();
                }
                std::cmp::Ordering::Less => return Some(EnvMismatch::Removed { name: sn.clone() }),
                std::cmp::Ordering::Greater => {
                    return Some(EnvMismatch::Added { name: cn.clone() });
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
    use std::ffi::{OsStr, OsString};

    use super::*;

    #[cfg(unix)]
    fn non_utf8_os_string() -> OsString {
        use std::os::unix::ffi::OsStringExt;

        OsString::from_vec(vec![0xFF])
    }

    #[cfg(windows)]
    fn non_utf8_os_string() -> OsString {
        use std::os::windows::ffi::OsStringExt;

        OsString::from_wide(&[0xD800])
    }

    #[test]
    fn validate_errors_on_current_non_utf8_tracked_env_value() {
        let mut tracked_envs = BTreeMap::new();
        tracked_envs.insert(Str::from("PROBE_ENV"), None);
        let fingerprint = PostRunFingerprint { tracked_envs, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_ENV")),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let workspace_root = vite_path::current_dir().expect("cwd");
        let err = fingerprint
            .validate(&workspace_root, &unfiltered_envs)
            .expect_err("non-UTF-8 tracked env values must error");

        assert!(err.to_string().contains("tracked env value for PROBE_ENV is not valid UTF-8"));
    }

    #[test]
    fn validate_reports_current_non_utf8_tracked_env_glob_value_as_changed() {
        let mut tracked_env_globs = BTreeMap::new();
        tracked_env_globs.insert(Str::from("PROBE_*"), BTreeMap::new());
        let fingerprint = PostRunFingerprint { tracked_env_globs, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_BAD")),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let workspace_root = vite_path::current_dir().expect("cwd");
        let mismatch =
            fingerprint.validate(&workspace_root, &unfiltered_envs).expect("validation succeeds");

        match mismatch {
            Some(PostRunMismatch::TrackedEnvGlob {
                pattern,
                mismatch: EnvMismatch::Changed { name },
            }) => {
                assert_eq!(pattern.as_str(), "PROBE_*");
                assert_eq!(name.as_str(), "PROBE_BAD");
            }
            other => panic!("expected changed tracked env glob mismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_ignores_non_utf8_tracked_env_glob_names() {
        let mut tracked_env_globs = BTreeMap::new();
        tracked_env_globs.insert(Str::from("PROBE_*"), BTreeMap::new());
        let fingerprint = PostRunFingerprint { tracked_env_globs, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(non_utf8_os_string()),
            Arc::<OsStr>::from(OsStr::new("value")),
        );

        let workspace_root = vite_path::current_dir().expect("cwd");
        let mismatch =
            fingerprint.validate(&workspace_root, &unfiltered_envs).expect("validation succeeds");

        assert!(mismatch.is_none());
    }
}

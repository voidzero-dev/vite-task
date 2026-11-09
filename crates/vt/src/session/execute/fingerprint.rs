//! Post-run fingerprinting for execution caching.
//!
//! This module provides types and functions for creating and validating
//! fingerprints of file system state after task execution.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::File,
    io::{self, BufRead},
    sync::Arc,
    time::Instant,
};

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use vt_casefold::EnvName;
use vt_glob::path::PathGlobSet;
use vt_path::{AbsolutePath, RelativePath, RelativePathBuf};
use vt_plan::cache_metadata::EnvValueHash;
use vt_str::Str;
use wincode::{SchemaRead, SchemaWrite};

use super::rebuildable::Rebuildable;
use crate::{
    collections::HashMap,
    session::cache::{EnvMismatch, InputChangeKind},
};

#[derive(
    SchemaWrite, SchemaRead, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum TrackedEnvQuery {
    Glob(Str),
    Prefix(Str),
}

/// Path read access info
#[derive(Debug, Clone, Copy)]
pub struct PathRead {
    pub read_dir_entries: bool,
}

/// Post-run fingerprint capturing file state after execution.
/// Used to validate whether cached outputs are still valid.
#[derive(SchemaWrite, SchemaRead, Debug, Default, Serialize)]
pub struct PostRunFingerprint {
    /// Paths inferred from fspy during execution, fingerprinted as the task
    /// left them. Only populated when `input_config.includes_auto` is true.
    pub inferred_inputs: HashMap<RelativePathBuf, InferredInput>,

    /// Env vars observed via runner-aware IPC `getEnv` with `tracked: true`.
    /// Key is the env name; value is the env value hash at execution time, or
    /// `None` if unset. Validated at cache lookup against the same plan env
    /// context that served the original request.
    pub tracked_envs: BTreeMap<Str, Option<EnvValueHash>>,

    /// Bulk env queries (`getEnvs`) made with `tracked: true`.
    /// Outer key is the query, inner map is the match-set at execution time
    /// (name -> value hash). Validated at cache lookup by re-matching against
    /// the current env context and comparing the resulting set.
    ///
    /// Non-UTF-8 env names are never matched, saved, or treated as errors:
    /// they are not returned to the client, so their existence cannot affect
    /// task behavior. Values are stricter. A matched env must have a UTF-8
    /// value; the JS client errors when querying a matched non-UTF-8 value,
    /// and cache-hit validation treats a currently matched non-UTF-8 value as
    /// a changed mismatch so stale cached output is not replayed.
    pub tracked_env_queries: BTreeMap<TrackedEnvQuery, BTreeMap<Str, EnvValueHash>>,
}

/// A mismatch between the stored post-run fingerprint and the current state.
#[derive(Debug, Clone)]
pub enum PostRunMismatch {
    /// An inferred input file or directory changed.
    Input { kind: InputChangeKind, path: RelativePathBuf },
    /// A tool-tracked env var changed value, appeared, or disappeared.
    TrackedEnv(EnvMismatch),
    /// A tool-tracked bulk env query's match-set changed between runs. Carries
    /// the first differing entry in env-name order.
    TrackedEnvQuery { query: TrackedEnvQuery, mismatch: EnvMismatch },
}

/// A path the task read, as the task left it.
#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct InferredInput {
    pub fingerprint: PathFingerprint,
    /// The task wrote the path, so a later run may find it missing, as on a
    /// clean checkout. A path that is there must still match `fingerprint`.
    pub may_be_missing: bool,
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
    /// `Folder(Some(_))` contains the directory entries.
    Folder(Option<DirListing>),
}

/// The entries of a listed directory.
#[derive(SchemaWrite, SchemaRead, PartialEq, Eq, Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    /// The entries sorted by name, without those in `ignored_entries` and
    /// those the input negative globs exclude.
    pub entries: BTreeMap<Str, DirEntryKind>,
    /// Names left out of `entries` that a later run may find missing or
    /// present: the entries the task can rebuild, and those that
    /// tool-reported `ignoreInput` paths name, whether or not they were there
    /// after the run. They're left out of the current listing before
    /// comparing, and so are the entries the input negative globs exclude,
    /// which aren't recorded: the globs are part of the cache key, so
    /// validation has the same ones.
    pub ignored_entries: BTreeSet<Str>,
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
    /// Input negative globs and tool-reported `ignoreInput` paths have
    /// already been applied to `inferred_path_reads` (see
    /// `cache_update::observe_fspy`). Paths already present in
    /// `globbed_inputs` are skipped — they are already tracked by the prerun
    /// glob fingerprint. Empty folders the task made and folders holding only
    /// what the task can rebuild are skipped too: a hit restores them if
    /// they're among the task's outputs. Every other path is fingerprinted as
    /// the task left it, including a file the task rewrote, except that a
    /// folder listing leaves out the entries the task can rebuild and those
    /// that `input_negative_globs` or `ignored_inputs` exclude, and records
    /// the names validation can't tell from the globs (see [`DirListing`]).
    ///
    /// # Arguments
    /// * `inferred_path_reads` - Map of paths that were read during execution (from fspy)
    /// * `rebuildable` - What the task can rebuild, so a later run may find it missing
    /// * `input_negative_globs` - The input negative globs, which also exclude listing entries
    /// * `ignored_inputs` - Tool-reported `ignoreInput` paths, which also exclude listing entries
    /// * `base_dir` - Workspace root for resolving relative paths
    /// * `globbed_inputs` - Prerun glob fingerprint; paths here are skipped
    /// * `tracked_envs` - Tool-requested env vars (name -> value hash), validated on lookup
    /// * `tracked_env_queries` - Tool-requested bulk env queries (query -> match-set hashes)
    #[expect(
        clippy::too_many_arguments,
        reason = "listing entries need the input exclusions next to the run's records"
    )]
    #[tracing::instrument(level = "debug", skip_all, name = "create_post_run_fingerprint")]
    pub fn create(
        inferred_path_reads: &HashMap<RelativePathBuf, PathRead>,
        rebuildable: &Rebuildable,
        input_negative_globs: &PathGlobSet<'_>,
        ignored_inputs: &FxHashSet<RelativePathBuf>,
        base_dir: &AbsolutePath,
        globbed_inputs: &BTreeMap<RelativePathBuf, u64>,
        tracked_envs: BTreeMap<Str, Option<EnvValueHash>>,
        tracked_env_queries: BTreeMap<TrackedEnvQuery, BTreeMap<Str, EnvValueHash>>,
    ) -> anyhow::Result<Self> {
        let start = Instant::now();
        // Rayon's threads don't run in this thread's span, so the per-input
        // events name it as their parent, which keeps them under the task.
        let span = tracing::Span::current();
        let inferred_inputs = inferred_path_reads
            .par_iter()
            .filter(|(path, _)| {
                !globbed_inputs.contains_key(*path) && !rebuildable.is_rebuildable_folder(path)
            })
            .map(|(relative_path, path_read)| {
                let full_path = Arc::<AbsolutePath>::from(base_dir.join(relative_path));
                let mut fingerprint = fingerprint_path(&full_path, *path_read)?;
                tracing::trace!(
                    parent: &span,
                    path = relative_path.as_str(),
                    "fingerprinted an inferred input"
                );
                if let PathFingerprint::Folder(Some(listing)) = &mut fingerprint {
                    if !rebuildable.is_empty() {
                        let DirListing { entries, ignored_entries } = listing;
                        entries.retain(|name, kind| {
                            let is_dir = matches!(kind, DirEntryKind::Dir);
                            let ignored =
                                rebuildable.is_rebuildable_entry(relative_path, name, is_dir);
                            if ignored {
                                ignored_entries.insert(name.clone());
                            }
                            !ignored
                        });
                    }
                    listing
                        .ignored_entries
                        .extend(ignored_input_names(relative_path, ignored_inputs));
                    leave_out_excluded_entries(listing, relative_path, input_negative_globs);
                }
                let may_be_missing =
                    rebuildable.may_be_missing(relative_path, &fingerprint, base_dir);
                Ok((relative_path.clone(), InferredInput { fingerprint, may_be_missing }))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        tracing::debug!(
            reads = inferred_path_reads.len(),
            inputs = inferred_inputs.len(),
            elapsed = ?start.elapsed(),
            "created the post-run fingerprint"
        );

        Ok(Self { inferred_inputs, tracked_envs, tracked_env_queries })
    }

    /// Validates the fingerprint against current filesystem state and the
    /// unfiltered env context used by runner-aware IPC. `unfiltered_envs` must
    /// be the same plan env context that served the original `getEnv` request,
    /// not the filtered env passed to the spawned process.
    /// `input_negative_globs` are the task's input negative globs, part of the
    /// cache key and so the same as when the fingerprint was created.
    ///
    /// Returns `Some(mismatch)` if anything changed, `None` if all valid.
    /// Returns an error if a tracked env is currently present but cannot be
    /// represented as UTF-8; treating that value as unset would make cache
    /// validation unsound.
    #[tracing::instrument(level = "debug", skip_all, name = "validate_post_run_fingerprint")]
    pub fn validate(
        &self,
        base_dir: &AbsolutePath,
        input_negative_globs: &PathGlobSet<'_>,
        unfiltered_envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
    ) -> anyhow::Result<Option<PostRunMismatch>> {
        let start = Instant::now();
        // As in `create`, the per-input events name this span as their parent.
        let span = tracing::Span::current();
        let input_mismatch =
            self.inferred_inputs.par_iter().find_map_any(|(input_relative_path, input)| {
                let InferredInput { fingerprint: path_fingerprint, may_be_missing } = input;
                let input_full_path = Arc::<AbsolutePath>::from(base_dir.join(input_relative_path));
                let path_read = PathRead {
                    read_dir_entries: matches!(path_fingerprint, PathFingerprint::Folder(Some(_))),
                };
                let mut current_path_fingerprint =
                    match fingerprint_path(&input_full_path, path_read) {
                        Ok(ok) => ok,
                        Err(err) => return Some(Err(err)),
                    };
                // The ignored entries may be missing or present, so the current
                // listing records and leaves out the same names, and the entries
                // the input negative globs exclude.
                if let (
                    PathFingerprint::Folder(Some(stored)),
                    PathFingerprint::Folder(Some(current)),
                ) = (path_fingerprint, &mut current_path_fingerprint)
                {
                    current.ignored_entries.clone_from(&stored.ignored_entries);
                    leave_out_excluded_entries(current, input_relative_path, input_negative_globs);
                }
                let unchanged = path_fingerprint == &current_path_fingerprint
                    || (*may_be_missing && current_path_fingerprint == PathFingerprint::NotFound);
                tracing::trace!(
                    parent: &span,
                    path = input_relative_path.as_str(),
                    unchanged,
                    "validated an inferred input"
                );
                if unchanged {
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
            });
        tracing::debug!(
            inputs = self.inferred_inputs.len(),
            valid = input_mismatch.is_none(),
            elapsed = ?start.elapsed(),
            "validated the inferred inputs"
        );
        if let Some(result) = input_mismatch {
            return result.map(Some);
        }

        for (name, stored_value) in &self.tracked_envs {
            let current_value = unfiltered_envs
                .get(EnvName::from_ref(OsStr::new(name.as_str())))
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

        for (query, stored_matches) in &self.tracked_env_queries {
            let current_matches = match match_env_query(query, unfiltered_envs)? {
                EnvQueryValidation::Matches(matches) => matches,
                EnvQueryValidation::NonUtf8Value(mismatch) => {
                    return Ok(Some(PostRunMismatch::TrackedEnvQuery {
                        query: query.clone(),
                        mismatch,
                    }));
                }
            };
            if let Some(mismatch) = first_env_glob_mismatch(stored_matches, &current_matches) {
                return Ok(Some(PostRunMismatch::TrackedEnvQuery {
                    query: query.clone(),
                    mismatch,
                }));
            }
        }

        Ok(None)
    }
}

/// Build the current match-set for `query` by enumerating the given env
/// snapshot and keeping matching UTF-8 names. If a matching env has a non-UTF-8
/// value, return a changed mismatch so the stale cache entry is not replayed.
fn match_env_query(
    query: &TrackedEnvQuery,
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
) -> anyhow::Result<EnvQueryValidation> {
    Ok(match query {
        TrackedEnvQuery::Glob(pattern) => {
            let glob = vt_glob::env::EnvGlob::new(pattern.as_str())?;
            collect_matching_envs(envs, |name| glob.is_match(name))
        }
        TrackedEnvQuery::Prefix(prefix) => {
            collect_matching_envs(envs, |name| EnvName::from_ref(name).starts_with(prefix.as_str()))
        }
    })
}

fn collect_matching_envs(
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
    is_match: impl Fn(&str) -> bool,
) -> EnvQueryValidation {
    let mut matches = BTreeMap::new();
    for (name, value) in envs {
        let Some(name_str) = name.inner().to_str() else {
            continue;
        };
        if !is_match(name_str) {
            continue;
        }
        let Some(value_str) = value.to_str() else {
            return EnvQueryValidation::NonUtf8Value(EnvMismatch::Changed {
                name: Str::from(name_str),
            });
        };
        matches.insert(Str::from(name_str), EnvValueHash::new(value_str));
    }
    EnvQueryValidation::Matches(matches)
}

enum EnvQueryValidation {
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
    old: Option<&'a DirListing>,
    new: Option<&'a DirListing>,
) -> (InputChangeKind, Option<&'a Str>) {
    let (Some(old_listing), Some(new_listing)) = (old, new) else {
        return (InputChangeKind::Added, None);
    };

    let mut old_iter = old_listing.entries.iter();
    let mut new_iter = new_listing.entries.iter();
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

/// The names of the entries of the folder `dir` that `ignored_inputs`,
/// tool-reported `ignoreInput` paths, cover, whether or not they're there. A
/// path covers itself and everything under it, but a listed folder is never
/// covered, since covered reads are left out, so an entry of `dir` is covered
/// only by a path naming it.
fn ignored_input_names<'a>(
    dir: &'a RelativePath,
    ignored_inputs: &'a FxHashSet<RelativePathBuf>,
) -> impl Iterator<Item = Str> + 'a {
    ignored_inputs.iter().filter(move |path| path.parent() == Some(dir)).map(|path| {
        let path = path.as_str();
        Str::from(path.rsplit_once('/').map_or(path, |(_, name)| name))
    })
}

/// Leaves out of `listing`, a listing of the folder `dir`, the entries named
/// in its `ignored_entries` and those whose path `input_negative_globs`
/// match. A folder entry is matched by its own path: `build/**` matches the
/// `build` folder itself, since `**` matches zero or more components, so it
/// leaves out the entry, while `build/*` or `build/**/*.js` exclude only
/// what is in the folder and leave the entry in.
fn leave_out_excluded_entries(
    listing: &mut DirListing,
    dir: &RelativePath,
    input_negative_globs: &PathGlobSet<'_>,
) {
    let DirListing { entries, ignored_entries } = listing;
    if ignored_entries.is_empty() && input_negative_globs.is_empty() {
        return;
    }
    entries.retain(|name, _| {
        !ignored_entries.contains(name)
            && (input_negative_globs.is_empty()
                || !RelativePathBuf::new(name.as_str())
                    .is_ok_and(|name| input_negative_globs.is_match(dir.join(name))))
    });
}

/// Check if a directory entry should be ignored in fingerprinting
const fn should_ignore_entry(name: &[u8]) -> bool {
    matches!(name, b"." | b".." | b".DS_Store")
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
    Ok(PathFingerprint::FileContentHash(super::hash::hash_file_content(path, reader)?))
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

    Ok(PathFingerprint::Folder(Some(DirListing { entries, ignored_entries: BTreeSet::new() })))
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
            Some(_) => DirEntryKind::File,
            // The file system doesn't report entry types: stat the entry
            // without following a symlink, as std does, so this agrees with
            // `Rebuildable`'s folder climb.
            None => {
                use nix::{
                    fcntl::AtFlags,
                    sys::stat::{SFlag, fstatat},
                };

                let stat = fstatat(fd, entry.file_name(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
                let file_type = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
                if file_type == SFlag::S_IFDIR {
                    DirEntryKind::Dir
                } else if file_type == SFlag::S_IFLNK {
                    DirEntryKind::Symlink
                } else {
                    DirEntryKind::File
                }
            }
        };

        #[expect(
            clippy::disallowed_types,
            reason = "from_utf8_lossy returns Cow referencing String"
        )]
        let name_str = String::from_utf8_lossy(name);
        entries.insert(Str::from(name_str.as_ref()), kind);
    }

    Ok(PathFingerprint::Folder(Some(DirListing { entries, ignored_entries: BTreeSet::new() })))
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};

    use rustc_hash::FxHashSet;
    use vt_path::AbsolutePathBuf;

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
            EnvName::new(Arc::<OsStr>::from(OsStr::new("PROBE_ENV"))),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let err = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect_err("non-UTF-8 tracked env values must error");

        assert!(err.to_string().contains("tracked env value for PROBE_ENV is not valid UTF-8"));
    }

    #[test]
    fn validate_reports_current_non_utf8_tracked_env_glob_value_as_changed() {
        let mut tracked_env_queries = BTreeMap::new();
        tracked_env_queries.insert(TrackedEnvQuery::Glob(Str::from("PROBE_*")), BTreeMap::new());
        let fingerprint =
            PostRunFingerprint { tracked_env_queries, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("PROBE_BAD"))),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let mismatch = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect("validation succeeds");

        match mismatch {
            Some(PostRunMismatch::TrackedEnvQuery {
                query,
                mismatch: EnvMismatch::Changed { name },
            }) => {
                assert_eq!(query, TrackedEnvQuery::Glob(Str::from("PROBE_*")));
                assert_eq!(name.as_str(), "PROBE_BAD");
            }
            other => panic!("expected changed tracked env query mismatch, got {other:?}"),
        }
    }

    #[test]
    fn validate_tracked_env_prefix_treats_star_literally() {
        let mut tracked_env_queries = BTreeMap::new();
        let mut stored_matches = BTreeMap::new();
        stored_matches.insert(Str::from("PROBE_*A"), EnvValueHash::new("literal"));
        tracked_env_queries.insert(TrackedEnvQuery::Prefix(Str::from("PROBE_*")), stored_matches);
        let fingerprint =
            PostRunFingerprint { tracked_env_queries, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("PROBE_*A"))),
            Arc::<OsStr>::from(OsStr::new("literal")),
        );
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("PROBE_XA"))),
            Arc::<OsStr>::from(OsStr::new("wildcard if interpreted as glob")),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let mismatch = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect("validation succeeds");

        assert!(mismatch.is_none());
    }

    #[test]
    fn validate_ignores_non_utf8_tracked_env_glob_names() {
        let mut tracked_env_queries = BTreeMap::new();
        tracked_env_queries.insert(TrackedEnvQuery::Glob(Str::from("PROBE_*")), BTreeMap::new());
        let fingerprint =
            PostRunFingerprint { tracked_env_queries, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(non_utf8_os_string())),
            Arc::<OsStr>::from(OsStr::new("value")),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let mismatch = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect("validation succeeds");

        assert!(mismatch.is_none());
    }

    #[test]
    fn validate_looks_up_tracked_env_names_by_platform_rules() {
        let mut tracked_envs = BTreeMap::new();
        tracked_envs.insert(Str::from("PROBE_ENV"), Some(EnvValueHash::new("value")));
        let fingerprint = PostRunFingerprint { tracked_envs, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("probe_env"))),
            Arc::<OsStr>::from(OsStr::new("value")),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let mismatch = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect("validation succeeds");

        assert_eq!(mismatch.is_none(), cfg!(windows));
    }

    #[test]
    fn validate_matches_tracked_env_prefix_by_platform_rules() {
        let mut tracked_env_queries = BTreeMap::new();
        let mut stored_matches = BTreeMap::new();
        stored_matches.insert(Str::from("probe_a"), EnvValueHash::new("value"));
        tracked_env_queries.insert(TrackedEnvQuery::Prefix(Str::from("PROBE_")), stored_matches);
        let fingerprint =
            PostRunFingerprint { tracked_env_queries, ..PostRunFingerprint::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("probe_a"))),
            Arc::<OsStr>::from(OsStr::new("value")),
        );

        let workspace_root = vt_path::current_dir().expect("cwd");
        let mismatch = fingerprint
            .validate(&workspace_root, &PathGlobSet::default(), &unfiltered_envs)
            .expect("validation succeeds");

        assert_eq!(mismatch.is_none(), cfg!(windows));
    }

    #[test]
    fn validate_allows_a_written_input_to_be_missing_but_not_changed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let path = RelativePathBuf::new("state.txt").unwrap();
        let full_path = workspace_root.join(&path);
        std::fs::write(full_path.as_path(), "v1").unwrap();

        let path_reads = HashMap::from([(path.clone(), PathRead { read_dir_entries: false })]);
        let create = |written| {
            PostRunFingerprint::create(
                &path_reads,
                &Rebuildable::new(
                    written,
                    FxHashSet::default(),
                    FxHashSet::default(),
                    &workspace_root,
                ),
                &PathGlobSet::default(),
                &FxHashSet::default(),
                &workspace_root,
                &BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
            )
            .unwrap()
        };
        let written = create(FxHashSet::from_iter([path]));
        let read_only = create(FxHashSet::default());
        let validate = |fingerprint: &PostRunFingerprint| {
            fingerprint
                .validate(&workspace_root, &PathGlobSet::default(), &FxHashMap::default())
                .unwrap()
        };

        std::fs::remove_file(full_path.as_path()).unwrap();
        assert!(validate(&written).is_none());
        assert!(matches!(
            validate(&read_only),
            Some(PostRunMismatch::Input { kind: InputChangeKind::Removed, .. })
        ));

        std::fs::write(full_path.as_path(), "garbage").unwrap();
        assert!(matches!(
            validate(&written),
            Some(PostRunMismatch::Input { kind: InputChangeKind::ContentModified, .. })
        ));
    }

    #[test]
    fn create_skips_folders_holding_only_written_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        for dir in ["out/sub", "mixed", "holds_empty/empty"] {
            std::fs::create_dir_all(workspace_root.join(dir).as_path()).unwrap();
        }
        let written = ["out/a.txt", "out/sub/b.txt", "mixed/a.txt", "holds_empty/a.txt"];
        for file in written.iter().copied().chain(["out/.DS_Store", "mixed/notes.txt"]) {
            std::fs::write(workspace_root.join(file).as_path(), "x").unwrap();
        }

        let path_reads = ["out", "out/sub", "mixed", "holds_empty"]
            .into_iter()
            .map(|dir| (rel(dir), PathRead { read_dir_entries: true }))
            .collect();
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &Rebuildable::new(
                written.into_iter().map(rel).collect(),
                FxHashSet::default(),
                FxHashSet::default(),
                &workspace_root,
            )
            .with_folders(&workspace_root),
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();

        let mut fingerprinted: Vec<&str> =
            fingerprint.inferred_inputs.keys().map(|path| path.as_str()).collect();
        fingerprinted.sort_unstable();
        assert_eq!(fingerprinted, ["holds_empty", "mixed"]);
    }

    #[test]
    fn folders_gone_after_the_run_play_no_part() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        std::fs::create_dir(workspace_root.join("out").as_path()).unwrap();
        std::fs::write(workspace_root.join("out/a.txt").as_path(), "x").unwrap();

        // `out/tmp` and the file the task wrote in it are gone after the run,
        // like Astro's `dist/.prerender`.
        let rebuildable = Rebuildable::new(
            ["out/a.txt", "out/tmp/b.txt"].into_iter().map(rel).collect(),
            FxHashSet::default(),
            FxHashSet::default(),
            &workspace_root,
        )
        .with_folders(&workspace_root);
        assert!(rebuildable.is_rebuildable_folder(&rel("out")));
        assert!(!rebuildable.is_rebuildable_folder(&rel("out/tmp")));
    }

    #[test]
    fn empty_folders_the_task_made_are_rebuildable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        for dir in ["out/empty", "out/drained", "mixed/made"] {
            std::fs::create_dir_all(workspace_root.join(dir).as_path()).unwrap();
        }
        for file in ["out/a.txt", "out/empty/.DS_Store", "mixed/made/notes.txt"] {
            std::fs::write(workspace_root.join(file).as_path(), "x").unwrap();
        }

        // `out/tmp` and the file the task wrote in it are gone after the run,
        // and so is the file it wrote in `out/drained`.
        let new = || {
            Rebuildable::new(
                ["out/a.txt", "out/tmp/b.txt", "out/drained/c.txt"].into_iter().map(rel).collect(),
                FxHashSet::default(),
                ["out", "out/empty", "out/tmp", "out/drained", "mixed/made"]
                    .into_iter()
                    .map(rel)
                    .collect(),
                &workspace_root,
            )
        };
        let empty_folders = FxHashSet::from_iter([rel("out/empty"), rel("out/drained")]);
        let rebuildable = new().with_folders(&workspace_root);
        assert_eq!(rebuildable.empty_folders(), &empty_folders);
        // Without climbing, the archive gets the same empty folders.
        assert_eq!(new().with_empty_folders(&workspace_root).empty_folders(), &empty_folders);

        let path_reads = ["out", "out/empty", "mixed", "mixed/made"]
            .into_iter()
            .map(|dir| (rel(dir), PathRead { read_dir_entries: false }))
            .collect();
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &rebuildable,
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        let mut fingerprinted: Vec<&str> =
            fingerprint.inferred_inputs.keys().map(|path| path.as_str()).collect();
        fingerprinted.sort_unstable();
        assert_eq!(fingerprinted, ["mixed", "mixed/made"]);
    }

    #[test]
    fn files_under_a_renamed_folder_count_as_written() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        std::fs::create_dir_all(workspace_root.join("deps/sub").as_path()).unwrap();
        for file in ["deps/a.js", "deps/sub/b.js", "deps/.DS_Store"] {
            std::fs::write(workspace_root.join(file).as_path(), "x").unwrap();
        }

        // `deps` was renamed into place, so only the folder was recorded. Its
        // `.DS_Store` doesn't count.
        let rebuildable = Rebuildable::new(
            FxHashSet::default(),
            FxHashSet::from_iter([rel("deps")]),
            FxHashSet::default(),
            &workspace_root,
        )
        .with_folders(&workspace_root);
        let mut written: Vec<&str> =
            rebuildable.written().iter().map(|path| path.as_str()).collect();
        written.sort_unstable();
        assert_eq!(written, ["deps/a.js", "deps/sub/b.js"]);

        let path_reads = HashMap::from([
            (rel("deps"), PathRead { read_dir_entries: true }),
            (rel("deps/a.js"), PathRead { read_dir_entries: false }),
        ]);
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &rebuildable,
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        let fingerprinted: Vec<&str> =
            fingerprint.inferred_inputs.keys().map(|path| path.as_str()).collect();
        assert_eq!(fingerprinted, ["deps/a.js"]);
        assert!(fingerprint.inferred_inputs[&rel("deps/a.js")].may_be_missing);
    }

    #[test]
    fn a_gone_folder_the_task_wrote_in_counts_as_written() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        // Vite's dependency optimizer probes a fresh folder name, makes the
        // folder, writes in it, and renames it into place. fspy records the
        // probe as a read, the folder as made, the file as a write, and
        // `.vite/deps` as the rename target, but nothing for the rename's
        // source.
        let full_path = |path: &str| workspace_root.join(rel(path));
        std::fs::create_dir_all(full_path(".vite/deps_temp_1a2b").as_path()).unwrap();
        std::fs::write(full_path(".vite/deps_temp_1a2b/package.json").as_path(), "{}").unwrap();
        std::fs::write(full_path(".vite/notes.txt").as_path(), "").unwrap();
        std::fs::rename(
            full_path(".vite/deps_temp_1a2b").as_path(),
            full_path(".vite/deps").as_path(),
        )
        .unwrap();

        let path_reads = [".vite", ".vite/deps_temp_1a2b", ".vite/probe"]
            .map(|read| (rel(read), PathRead { read_dir_entries: false }));
        let fingerprint = PostRunFingerprint::create(
            &HashMap::from(path_reads),
            &Rebuildable::new(
                FxHashSet::from_iter([rel(".vite/deps_temp_1a2b/package.json")]),
                FxHashSet::from_iter([rel(".vite/deps")]),
                FxHashSet::from_iter([rel(".vite/deps_temp_1a2b")]),
                &workspace_root,
            )
            .with_folders(&workspace_root),
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();

        let may_be_missing = |read: &str| fingerprint.inferred_inputs[&rel(read)].may_be_missing;
        assert!(may_be_missing(".vite/deps_temp_1a2b"));
        // A folder that is still there with other files, or a path the task
        // only probed, isn't.
        assert!(!may_be_missing(".vite"));
        assert!(!may_be_missing(".vite/probe"));
    }

    #[test]
    fn a_write_record_on_a_folder_is_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        std::fs::create_dir_all(workspace_root.join("deps").as_path()).unwrap();
        std::fs::write(workspace_root.join("deps/a.js").as_path(), "x").unwrap();

        // An open of `deps` with write access puts nothing under it, and a read
        // of it must still find it.
        let rebuildable = Rebuildable::new(
            FxHashSet::from_iter([rel("deps")]),
            FxHashSet::default(),
            FxHashSet::default(),
            &workspace_root,
        )
        .with_folders(&workspace_root);
        let file = PathFingerprint::FileContentHash(0);
        assert!(!rebuildable.may_be_missing(&rel("deps/a.js"), &file, &workspace_root));
        assert!(!rebuildable.may_be_missing(
            &rel("deps"),
            &PathFingerprint::Folder(None),
            &workspace_root
        ));
        assert!(!rebuildable.is_rebuildable_folder(&rel("deps")));
        let path_reads = HashMap::from([(rel("deps"), PathRead { read_dir_entries: false })]);
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &rebuildable,
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        assert!(!fingerprint.inferred_inputs[&rel("deps")].may_be_missing);
    }

    #[test]
    fn listings_leave_out_entries_the_task_can_rebuild() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        for dir in ["pkg/gen", "pkg/mixed"] {
            std::fs::create_dir_all(workspace_root.join(dir).as_path()).unwrap();
        }
        let written = ["pkg/gen/a.txt", "pkg/out.txt", "pkg/mixed/a.txt"];
        for file in written.iter().copied().chain(["pkg/mixed/notes.txt"]) {
            std::fs::write(workspace_root.join(file).as_path(), "x").unwrap();
        }

        let path_reads = HashMap::from([(rel("pkg"), PathRead { read_dir_entries: true })]);
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &Rebuildable::new(
                written.into_iter().map(rel).collect(),
                FxHashSet::default(),
                FxHashSet::default(),
                &workspace_root,
            )
            .with_folders(&workspace_root),
            &PathGlobSet::default(),
            &FxHashSet::default(),
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            fingerprint.inferred_inputs[&rel("pkg")].fingerprint,
            PathFingerprint::Folder(Some(DirListing {
                entries: BTreeMap::from([("mixed".into(), DirEntryKind::Dir)]),
                ignored_entries: BTreeSet::from(["gen".into(), "out.txt".into()]),
            }))
        );

        let validate = || {
            fingerprint
                .validate(&workspace_root, &PathGlobSet::default(), &FxHashMap::default())
                .unwrap()
        };
        assert!(validate().is_none());
        std::fs::remove_dir_all(workspace_root.join("pkg/gen").as_path()).unwrap();
        std::fs::remove_file(workspace_root.join("pkg/out.txt").as_path()).unwrap();
        assert!(validate().is_none());

        std::fs::write(workspace_root.join("pkg/notes.txt").as_path(), "x").unwrap();
        match validate() {
            Some(PostRunMismatch::Input { kind: InputChangeKind::Added, path }) => {
                assert_eq!(path.as_str(), "pkg/notes.txt");
            }
            other => panic!("expected `pkg/notes.txt` added, got {other:?}"),
        }
    }

    #[test]
    fn listings_leave_out_entries_the_input_config_excludes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        let rel = |path: &str| RelativePathBuf::new(path).unwrap();
        for dir in ["pkg/gen", "pkg/src"] {
            std::fs::create_dir_all(workspace_root.join(dir).as_path()).unwrap();
        }
        std::fs::write(workspace_root.join("pkg/tool.log").as_path(), "x").unwrap();

        // `pkg/out/**` matches `pkg/out` itself, `pkg/gen/*` only what is in
        // `pkg/gen`. The tool reported `pkg/tool.log`, which is there, and
        // `pkg/.cache`, which isn't, as ignored inputs.
        let input_negative_globs = PathGlobSet::new(&["pkg/out/**", "pkg/gen/*"]).unwrap();
        let ignored_inputs =
            FxHashSet::from_iter([rel("pkg/tool.log"), rel("pkg/.cache"), rel("other/x")]);
        let path_reads = HashMap::from([(rel("pkg"), PathRead { read_dir_entries: true })]);
        let fingerprint = PostRunFingerprint::create(
            &path_reads,
            &Rebuildable::default(),
            &input_negative_globs,
            &ignored_inputs,
            &workspace_root,
            &BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            fingerprint.inferred_inputs[&rel("pkg")].fingerprint,
            PathFingerprint::Folder(Some(DirListing {
                entries: BTreeMap::from([
                    ("gen".into(), DirEntryKind::Dir),
                    ("src".into(), DirEntryKind::Dir),
                ]),
                ignored_entries: BTreeSet::from([".cache".into(), "tool.log".into()]),
            }))
        );

        let validate = || {
            fingerprint
                .validate(&workspace_root, &input_negative_globs, &FxHashMap::default())
                .unwrap()
        };
        std::fs::create_dir(workspace_root.join("pkg/out").as_path()).unwrap();
        std::fs::create_dir(workspace_root.join("pkg/.cache").as_path()).unwrap();
        std::fs::remove_file(workspace_root.join("pkg/tool.log").as_path()).unwrap();
        assert!(validate().is_none());

        std::fs::write(workspace_root.join("pkg/notes.txt").as_path(), "x").unwrap();
        match validate() {
            Some(PostRunMismatch::Input { kind: InputChangeKind::Added, path }) => {
                assert_eq!(path.as_str(), "pkg/notes.txt");
            }
            other => panic!("expected `pkg/notes.txt` added, got {other:?}"),
        }
    }
}

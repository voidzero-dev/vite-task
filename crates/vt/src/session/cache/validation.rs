//! Cache entry validation and fallback diagnostics, independent of storage.

use std::{collections::BTreeMap, ffi::OsStr, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};
use vt_casefold::EnvName;
use vt_glob::path::PathGlobSet;
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_plan::cache_metadata::SpawnFingerprint;

use super::{
    CacheEntryKey, CacheEntryValue, Change, EnvMismatch, EnvSource, FileCheck, InputChangeKind,
};

impl CacheEntryValue {
    /// Compare the entry with the current execution: explicit inputs, then
    /// inferred inputs, and tracked environment values against
    /// `unfiltered_envs`, the execution's `CacheMetadata::unfiltered_envs`.
    /// `input_negative_globs`, compiled from the entry key's input config,
    /// leave entries out of listings. Returns every env change and, with
    /// [`FileCheck::First`], the first changed file found, or with
    /// [`FileCheck::All`], every changed file. Nothing when the entry is valid.
    ///
    /// # Errors
    ///
    /// Propagates errors from post-run fingerprint validation. Once a file
    /// changed, the entry misses whatever else changed, so an error checking
    /// the other files or the envs only leaves their changes out.
    pub(crate) fn validate(
        &self,
        unfiltered_envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
        globbed_inputs: &BTreeMap<RelativePathBuf, u64>,
        input_negative_globs: &PathGlobSet<'_>,
        workspace_root: &AbsolutePath,
        file_check: FileCheck,
    ) -> anyhow::Result<Vec<Change>> {
        // Without `-v`, the first changed file is enough to know it's a miss.
        let limit = if file_check == FileCheck::First { 1 } else { usize::MAX };
        let mut changes = globbed_input_changes(&self.globbed_inputs, globbed_inputs, limit);
        if changes.is_empty() || file_check == FileCheck::All {
            let input_changes = self.post_run_fingerprint.input_changes(
                workspace_root,
                input_negative_globs,
                file_check,
            );
            add_checked(&mut changes, input_changes, "inferred inputs")?;
        }
        let env_changes = self.post_run_fingerprint.env_changes(unfiltered_envs);
        add_checked(&mut changes, env_changes, "tracked envs")?;
        Ok(changes)
    }
}

/// Add to `changes` what a later check of `what` found, one result per change
/// or per thing it couldn't check, or fail with its first error. Once a
/// change is known, the entry misses whatever else changed, so an error then
/// only leaves out what that check would have found.
fn add_checked(
    changes: &mut Vec<Change>,
    mut checked: Vec<anyhow::Result<Change>>,
    what: &str,
) -> anyhow::Result<()> {
    // Changes first, so they're known before any error.
    checked.sort_by_key(Result::is_err);
    for checked in checked {
        match checked {
            Ok(change) => changes.push(change),
            Err(err) if !changes.is_empty() => {
                tracing::debug!(?err, "failed to check the {what} of a changed entry");
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

impl CacheEntryKey {
    /// Every difference between this stored key and `current`: the spawn
    /// fingerprint, then the input and output configuration. Explains a miss
    /// that found the task's previous entry under this key.
    pub(crate) fn changes_to(&self, current: &Self) -> Vec<Change> {
        // Destructure to ensure we handle all fields when new ones are added.
        let Self { spawn_fingerprint, input_config, output_config } = self;
        let mut changes = spawn_changes(spawn_fingerprint, &current.spawn_fingerprint);
        if *input_config != current.input_config {
            changes.push(Change::InputConfig);
        }
        if *output_config != current.output_config {
            changes.push(Change::OutputConfig);
        }
        changes
    }
}

/// Every difference between two spawn fingerprints.
fn spawn_changes(old: &SpawnFingerprint, new: &SpawnFingerprint) -> Vec<Change> {
    let mut changes = Vec::new();
    let old_env = old.env_fingerprints();
    let new_env = new.env_fingerprints();
    changes.extend(
        EnvMismatch::diff(&old_env.fingerprinted_envs, &new_env.fingerprinted_envs)
            .map(|mismatch| Change::Env { mismatch, source: EnvSource::Fingerprinted }),
    );

    // Check untracked env config changes
    let old_untracked: FxHashSet<_> = old_env.untracked_env_config.iter().collect();
    let new_untracked: FxHashSet<_> = new_env.untracked_env_config.iter().collect();
    for name in old_untracked.difference(&new_untracked) {
        changes.push(Change::UntrackedEnvRemoved { name: (*name).clone() });
    }
    for name in new_untracked.difference(&old_untracked) {
        changes.push(Change::UntrackedEnvAdded { name: (*name).clone() });
    }

    if old.program_fingerprint_debug() != new.program_fingerprint_debug() {
        changes.push(Change::Program);
    }
    if old.args() != new.args() {
        changes.push(Change::Args);
    }
    if old.cwd() != new.cwd() {
        changes.push(Change::Cwd);
    }

    changes
}

/// Compare stored and current globbed inputs, returning the first `limit`
/// changed paths, in path order. Both maps are `BTreeMap` so we iterate them
/// in sorted lockstep.
fn globbed_input_changes(
    stored: &BTreeMap<RelativePathBuf, u64>,
    current: &BTreeMap<RelativePathBuf, u64>,
    limit: usize,
) -> Vec<Change> {
    let mut changes = Vec::new();
    let mut stored_iter = stored.iter();
    let mut current_iter = current.iter();
    let mut s = stored_iter.next();
    let mut c = current_iter.next();
    let change = |kind, path: &RelativePathBuf| Change::File { kind, path: path.clone() };

    while changes.len() < limit {
        match (s, c) {
            (None, None) => break,
            (Some((sp, _)), None) => {
                changes.push(change(InputChangeKind::Removed, sp));
                s = stored_iter.next();
            }
            (None, Some((cp, _))) => {
                changes.push(change(InputChangeKind::Added, cp));
                c = current_iter.next();
            }
            (Some((sp, sh)), Some((cp, ch))) => match sp.cmp(cp) {
                std::cmp::Ordering::Equal => {
                    if sh != ch {
                        changes.push(change(InputChangeKind::ContentModified, sp));
                    }
                    s = stored_iter.next();
                    c = current_iter.next();
                }
                std::cmp::Ordering::Less => {
                    changes.push(change(InputChangeKind::Removed, sp));
                    s = stored_iter.next();
                }
                std::cmp::Ordering::Greater => {
                    changes.push(change(InputChangeKind::Added, cp));
                    c = current_iter.next();
                }
            },
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, time::Duration};

    use vt_str::Str;

    use super::*;
    use crate::session::execute::fingerprint::PostRunFingerprint;

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

    /// A tracked env that can't be compared fails the lookup only when the
    /// entry would otherwise be valid. After a changed file, it's a miss.
    #[test]
    fn env_that_cannot_be_compared_fails_only_a_valid_entry() {
        let path = RelativePathBuf::new("a.txt").unwrap();
        let value = CacheEntryValue {
            post_run_fingerprint: PostRunFingerprint {
                tracked_envs: BTreeMap::from([(Str::from("PROBE_ENV"), None)]),
                ..PostRunFingerprint::default()
            },
            std_outputs: Arc::new([]),
            duration: Duration::ZERO,
            globbed_inputs: BTreeMap::from([(path.clone(), 1)]),
            output_archive: None,
        };
        let unfiltered_envs = FxHashMap::from_iter([(
            EnvName::new(Arc::<OsStr>::from(OsStr::new("PROBE_ENV"))),
            Arc::<OsStr>::from(non_utf8_os_string()),
        )]);
        let workspace_root = vt_path::current_dir().unwrap();
        let validate = |inputs| {
            value.validate(
                &unfiltered_envs,
                &inputs,
                &PathGlobSet::default(),
                &workspace_root,
                FileCheck::First,
            )
        };

        assert!(validate(BTreeMap::from([(path.clone(), 1)])).is_err());
        let changes = validate(BTreeMap::from([(path.clone(), 2)])).unwrap();
        assert_eq!(changes, [Change::File { kind: InputChangeKind::ContentModified, path }]);
    }

    /// A tracked env that can't be compared after another one changed keeps
    /// that change: it's a miss, not a failed lookup.
    #[test]
    fn env_that_cannot_be_compared_keeps_another_envs_change() {
        let value = CacheEntryValue {
            post_run_fingerprint: PostRunFingerprint {
                tracked_envs: BTreeMap::from([
                    (Str::from("CHANGED_ENV"), None),
                    (Str::from("PROBE_ENV"), None),
                ]),
                ..PostRunFingerprint::default()
            },
            std_outputs: Arc::new([]),
            duration: Duration::ZERO,
            globbed_inputs: BTreeMap::new(),
            output_archive: None,
        };
        let env = |name: &str, value: OsString| {
            (EnvName::new(Arc::<OsStr>::from(OsStr::new(name))), Arc::<OsStr>::from(value))
        };
        let unfiltered_envs = FxHashMap::from_iter([
            env("CHANGED_ENV", OsString::from("set")),
            env("PROBE_ENV", non_utf8_os_string()),
        ]);
        let workspace_root = vt_path::current_dir().unwrap();
        let changes = value
            .validate(
                &unfiltered_envs,
                &BTreeMap::new(),
                &PathGlobSet::default(),
                &workspace_root,
                FileCheck::First,
            )
            .unwrap();
        assert_eq!(
            changes,
            [Change::Env {
                mismatch: EnvMismatch::Added { name: Str::from("CHANGED_ENV") },
                source: EnvSource::Tracked,
            }]
        );
    }

    /// Without `-v`, the file check stops at the first changed explicit
    /// input; with it, every one is listed in path order.
    #[test]
    fn every_changed_explicit_input_is_listed_when_checking_all() {
        let rel = |path| RelativePathBuf::new(path).unwrap();
        let value = CacheEntryValue {
            post_run_fingerprint: PostRunFingerprint::default(),
            std_outputs: Arc::new([]),
            duration: Duration::ZERO,
            globbed_inputs: BTreeMap::from([(rel("a.txt"), 1), (rel("b.txt"), 1)]),
            output_archive: None,
        };
        let current = BTreeMap::from([(rel("a.txt"), 2), (rel("c.txt"), 1)]);
        let workspace_root = vt_path::current_dir().unwrap();
        let validate = |file_check| {
            value
                .validate(
                    &FxHashMap::default(),
                    &current,
                    &PathGlobSet::default(),
                    &workspace_root,
                    file_check,
                )
                .unwrap()
        };
        let file = |kind, path| Change::File { kind, path: rel(path) };

        assert_eq!(validate(FileCheck::First), [file(InputChangeKind::ContentModified, "a.txt")]);
        assert_eq!(
            validate(FileCheck::All),
            [
                file(InputChangeKind::ContentModified, "a.txt"),
                file(InputChangeKind::Removed, "b.txt"),
                file(InputChangeKind::Added, "c.txt"),
            ]
        );
    }

    /// With `-v`, a listed folder that gains a valid entry and one whose name
    /// isn't a valid path keeps the valid one: it's a miss, not a failed
    /// lookup. Only Unix allows a `\` in a file name, so only Unix can test it.
    #[cfg(unix)]
    #[test]
    fn folder_entry_that_cannot_be_named_keeps_the_others() {
        use crate::session::execute::fingerprint::{DirListing, InferredInput, PathFingerprint};

        let tmp = tempfile::TempDir::new().unwrap();
        let workspace_root = vt_path::AbsolutePathBuf::new(tmp.path().to_path_buf()).unwrap();
        std::fs::create_dir(workspace_root.join("dir").as_path()).unwrap();
        std::fs::write(workspace_root.join("dir/a.txt").as_path(), "x").unwrap();
        std::fs::write(workspace_root.join("dir/a\\b").as_path(), "x").unwrap();
        let rel = |path| RelativePathBuf::new(path).unwrap();
        let listing = DirListing {
            entries: BTreeMap::new(),
            ignored_entries: std::collections::BTreeSet::new(),
        };
        let value = CacheEntryValue {
            post_run_fingerprint: PostRunFingerprint {
                inferred_inputs: crate::collections::HashMap::from_iter([(
                    rel("dir"),
                    InferredInput {
                        fingerprint: PathFingerprint::Folder(Some(listing)),
                        may_be_missing: false,
                    },
                )]),
                ..PostRunFingerprint::default()
            },
            std_outputs: Arc::new([]),
            duration: Duration::ZERO,
            globbed_inputs: BTreeMap::new(),
            output_archive: None,
        };
        let validate = |file_check| {
            value.validate(
                &FxHashMap::default(),
                &BTreeMap::new(),
                &PathGlobSet::default(),
                &workspace_root,
                file_check,
            )
        };

        let changes = validate(FileCheck::All).unwrap();
        assert_eq!(
            changes,
            [Change::File { kind: InputChangeKind::Added, path: rel("dir/a.txt") }]
        );
    }
}

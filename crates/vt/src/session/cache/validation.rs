//! Cache entry validation and fallback diagnostics, independent of storage.

use std::{collections::BTreeMap, ffi::OsStr, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};
use vt_casefold::EnvName;
use vt_glob::path::PathGlobSet;
use vt_path::{AbsolutePath, RelativePathBuf};
use vt_plan::cache_metadata::SpawnFingerprint;

use super::{CacheEntryKey, CacheEntryValue, Change, EnvMismatch, EnvSource, InputChangeKind};

impl CacheEntryValue {
    /// Compare the entry with the current execution: explicit inputs, then
    /// inferred inputs, and tracked environment values against
    /// `unfiltered_envs`, the execution's `CacheMetadata::unfiltered_envs`.
    /// `input_negative_globs`, compiled from the entry key's input config,
    /// leave entries out of listings. Returns every env change and the first
    /// changed file found, or nothing when the entry is valid.
    ///
    /// # Errors
    ///
    /// Propagates errors from post-run fingerprint validation. Once a file
    /// changed, the entry misses whatever the envs are, so an error comparing
    /// them only leaves the env changes out.
    pub(crate) fn validate(
        &self,
        unfiltered_envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
        globbed_inputs: &BTreeMap<RelativePathBuf, u64>,
        input_negative_globs: &PathGlobSet<'_>,
        workspace_root: &AbsolutePath,
    ) -> anyhow::Result<Vec<Change>> {
        let mut changes: Vec<Change> =
            detect_globbed_input_change(&self.globbed_inputs, globbed_inputs).into_iter().collect();
        if changes.is_empty() {
            changes.extend(
                self.post_run_fingerprint
                    .first_input_change(workspace_root, input_negative_globs)?,
            );
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

/// Compare stored and current globbed inputs, returning the first changed path.
/// Both maps are `BTreeMap` so we iterate them in sorted lockstep.
fn detect_globbed_input_change(
    stored: &BTreeMap<RelativePathBuf, u64>,
    current: &BTreeMap<RelativePathBuf, u64>,
) -> Option<Change> {
    let mut stored_iter = stored.iter();
    let mut current_iter = current.iter();
    let mut s = stored_iter.next();
    let mut c = current_iter.next();

    loop {
        match (s, c) {
            (None, None) => return None,
            (Some((sp, _)), None) => {
                return Some(Change::File { kind: InputChangeKind::Removed, path: sp.clone() });
            }
            (None, Some((cp, _))) => {
                return Some(Change::File { kind: InputChangeKind::Added, path: cp.clone() });
            }
            (Some((sp, sh)), Some((cp, ch))) => match sp.cmp(cp) {
                std::cmp::Ordering::Equal => {
                    if sh != ch {
                        return Some(Change::File {
                            kind: InputChangeKind::ContentModified,
                            path: sp.clone(),
                        });
                    }
                    s = stored_iter.next();
                    c = current_iter.next();
                }
                std::cmp::Ordering::Less => {
                    return Some(Change::File { kind: InputChangeKind::Removed, path: sp.clone() });
                }
                std::cmp::Ordering::Greater => {
                    return Some(Change::File { kind: InputChangeKind::Added, path: cp.clone() });
                }
            },
        }
    }
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
            value.validate(&unfiltered_envs, &inputs, &PathGlobSet::default(), &workspace_root)
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
            .validate(&unfiltered_envs, &BTreeMap::new(), &PathGlobSet::default(), &workspace_root)
            .unwrap();
        assert_eq!(
            changes,
            [Change::Env {
                mismatch: EnvMismatch::Added { name: Str::from("CHANGED_ENV") },
                source: EnvSource::Tracked,
            }]
        );
    }
}

//! Cache entry validation and fallback diagnostics, independent of storage.

use std::collections::BTreeMap;

use vt_path::{AbsolutePath, RelativePathBuf};
use vt_plan::cache_metadata::CacheMetadata;

use super::{CacheEntryKey, CacheEntryValue, FingerprintMismatch, InputChangeKind};

impl CacheEntryValue {
    /// Validate explicit inputs, then inferred inputs and tracked environment values.
    /// Returns the first mismatch, or `None` when the entry is valid.
    ///
    /// # Errors
    ///
    /// Propagates errors from post-run fingerprint validation.
    pub(crate) fn validate(
        &self,
        cache_metadata: &CacheMetadata,
        globbed_inputs: &BTreeMap<RelativePathBuf, u64>,
        workspace_root: &AbsolutePath,
    ) -> anyhow::Result<Option<FingerprintMismatch>> {
        if let Some(mismatch) = detect_globbed_input_change(&self.globbed_inputs, globbed_inputs) {
            return Ok(Some(mismatch));
        }

        self.post_run_fingerprint
            .validate(workspace_root, &cache_metadata.unfiltered_envs)
            .map(|mismatch| mismatch.map(FingerprintMismatch::from))
    }
}

impl CacheEntryKey {
    /// Explain a fallback miss by comparing the stored key with the current key.
    /// The keys must differ. Checks spawn, input, and output configuration in order.
    pub(crate) fn into_mismatch(self, current: &Self) -> FingerprintMismatch {
        // Destructure to ensure we handle all fields when new ones are added.
        let Self {
            spawn_fingerprint: old_spawn_fingerprint,
            input_config: old_input_config,
            output_config: old_output_config,
        } = self;
        if old_spawn_fingerprint != current.spawn_fingerprint {
            FingerprintMismatch::SpawnFingerprint {
                old: old_spawn_fingerprint,
                new: current.spawn_fingerprint.clone(),
            }
        } else if old_input_config != current.input_config {
            FingerprintMismatch::InputConfig
        } else {
            debug_assert_ne!(old_output_config, current.output_config);
            FingerprintMismatch::OutputConfig
        }
    }
}

/// Compare stored and current globbed inputs, returning the first changed path.
/// Both maps are `BTreeMap` so we iterate them in sorted lockstep.
fn detect_globbed_input_change(
    stored: &BTreeMap<RelativePathBuf, u64>,
    current: &BTreeMap<RelativePathBuf, u64>,
) -> Option<FingerprintMismatch> {
    let mut stored_iter = stored.iter();
    let mut current_iter = current.iter();
    let mut s = stored_iter.next();
    let mut c = current_iter.next();

    loop {
        match (s, c) {
            (None, None) => return None,
            (Some((sp, _)), None) => {
                return Some(FingerprintMismatch::InputChanged {
                    kind: InputChangeKind::Removed,
                    path: sp.clone(),
                });
            }
            (None, Some((cp, _))) => {
                return Some(FingerprintMismatch::InputChanged {
                    kind: InputChangeKind::Added,
                    path: cp.clone(),
                });
            }
            (Some((sp, sh)), Some((cp, ch))) => match sp.cmp(cp) {
                std::cmp::Ordering::Equal => {
                    if sh != ch {
                        return Some(FingerprintMismatch::InputChanged {
                            kind: InputChangeKind::ContentModified,
                            path: sp.clone(),
                        });
                    }
                    s = stored_iter.next();
                    c = current_iter.next();
                }
                std::cmp::Ordering::Less => {
                    return Some(FingerprintMismatch::InputChanged {
                        kind: InputChangeKind::Removed,
                        path: sp.clone(),
                    });
                }
                std::cmp::Ordering::Greater => {
                    return Some(FingerprintMismatch::InputChanged {
                        kind: InputChangeKind::Added,
                        path: cp.clone(),
                    });
                }
            },
        }
    }
}

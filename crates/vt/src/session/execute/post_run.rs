//! Post-run environment fingerprinting: env values and bulk env queries
//! observed by runner-aware tools during execution, validated again at cache
//! lookup. The filesystem half of post-run fingerprinting lives in
//! [`super::task_fs`].

use std::{collections::BTreeMap, ffi::OsStr, sync::Arc};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use vt_plan::cache_metadata::EnvValueHash;
use vt_str::Str;
use wincode::{SchemaRead, SchemaWrite};

use crate::session::cache::EnvMismatch;

#[derive(
    SchemaWrite, SchemaRead, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum TrackedEnvQuery {
    Glob(Str),
    Prefix(Str),
}

/// Env state observed by runner-aware tools during execution.
/// Used to validate whether cached outputs are still valid.
#[derive(SchemaWrite, SchemaRead, Debug, Default, Serialize)]
pub struct TrackedEnvFingerprints {
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

/// A mismatch between the stored tracked-env fingerprints and the current
/// environment.
#[derive(Debug, Clone)]
pub enum PostRunMismatch {
    /// A tool-tracked env var changed value, appeared, or disappeared.
    TrackedEnv(EnvMismatch),
    /// A tool-tracked bulk env query's match-set changed between runs. Carries
    /// the first differing entry in env-name order.
    TrackedEnvQuery { query: TrackedEnvQuery, mismatch: EnvMismatch },
}

impl TrackedEnvFingerprints {
    /// Validates the tracked env state against the unfiltered env context used
    /// by runner-aware IPC. `unfiltered_envs` must be the same plan env
    /// context that served the original `getEnv` request, not the filtered env
    /// passed to the spawned process.
    ///
    /// Returns `Some(mismatch)` if anything changed, `None` if all valid.
    /// Returns an error if a tracked env is currently present but cannot be
    /// represented as UTF-8; treating that value as unset would make cache
    /// validation unsound.
    #[tracing::instrument(level = "debug", skip_all, name = "validate_tracked_envs")]
    pub fn validate_envs(
        &self,
        unfiltered_envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    ) -> anyhow::Result<Option<PostRunMismatch>> {
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
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
) -> anyhow::Result<EnvQueryValidation> {
    Ok(match query {
        TrackedEnvQuery::Glob(pattern) => {
            let glob = vt_glob::env::EnvGlob::new(pattern.as_str())?;
            collect_matching_envs(envs, |name| glob.is_match(name))
        }
        TrackedEnvQuery::Prefix(prefix) => {
            collect_matching_envs(envs, |name| env_name_starts_with(name, prefix.as_str()))
        }
    })
}

fn collect_matching_envs(
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    is_match: impl Fn(&str) -> bool,
) -> EnvQueryValidation {
    let mut matches = BTreeMap::new();
    for (name, value) in envs {
        let Some(name_str) = name.to_str() else {
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

#[cfg(not(windows))]
fn env_name_starts_with(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix)
}

#[cfg(windows)]
fn env_name_starts_with(name: &str, prefix: &str) -> bool {
    let mut name_chars = name.chars();
    for prefix_char in prefix.chars() {
        let Some(name_char) = name_chars.next() else {
            return false;
        };
        if !name_char.eq_ignore_ascii_case(&prefix_char) {
            return false;
        }
    }
    true
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
        let fingerprints =
            TrackedEnvFingerprints { tracked_envs, ..TrackedEnvFingerprints::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_ENV")),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let err = fingerprints
            .validate_envs(&unfiltered_envs)
            .expect_err("non-UTF-8 tracked env values must error");

        assert!(err.to_string().contains("tracked env value for PROBE_ENV is not valid UTF-8"));
    }

    #[test]
    fn validate_reports_current_non_utf8_tracked_env_glob_value_as_changed() {
        let mut tracked_env_queries = BTreeMap::new();
        tracked_env_queries.insert(TrackedEnvQuery::Glob(Str::from("PROBE_*")), BTreeMap::new());
        let fingerprints =
            TrackedEnvFingerprints { tracked_env_queries, ..TrackedEnvFingerprints::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_BAD")),
            Arc::<OsStr>::from(non_utf8_os_string()),
        );

        let mismatch = fingerprints.validate_envs(&unfiltered_envs).expect("validation succeeds");

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
        let fingerprints =
            TrackedEnvFingerprints { tracked_env_queries, ..TrackedEnvFingerprints::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_*A")),
            Arc::<OsStr>::from(OsStr::new("literal")),
        );
        unfiltered_envs.insert(
            Arc::<OsStr>::from(OsStr::new("PROBE_XA")),
            Arc::<OsStr>::from(OsStr::new("wildcard if interpreted as glob")),
        );

        let mismatch = fingerprints.validate_envs(&unfiltered_envs).expect("validation succeeds");

        assert!(mismatch.is_none());
    }

    #[test]
    fn validate_ignores_non_utf8_tracked_env_glob_names() {
        let mut tracked_env_queries = BTreeMap::new();
        tracked_env_queries.insert(TrackedEnvQuery::Glob(Str::from("PROBE_*")), BTreeMap::new());
        let fingerprints =
            TrackedEnvFingerprints { tracked_env_queries, ..TrackedEnvFingerprints::default() };

        let mut unfiltered_envs = FxHashMap::default();
        unfiltered_envs.insert(
            Arc::<OsStr>::from(non_utf8_os_string()),
            Arc::<OsStr>::from(OsStr::new("value")),
        );

        let mismatch = fingerprints.validate_envs(&unfiltered_envs).expect("validation succeeds");

        assert!(mismatch.is_none());
    }
}

//! Remote cache settings resolved for each invocation during planning.

use std::{ffi::OsStr, sync::Arc};

use rustc_hash::FxHashMap;
use serde::Serialize;
use vt_casefold::EnvName;

use crate::Error;

pub(crate) const MODE_ENV: &str = "VP_REMOTE_CACHE";
const URL_ENV: &str = "VP_REMOTE_CACHE_URL";

/// Remote access requested by an invocation. Local cache policy is independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteCacheMode {
    Off,
    Read,
    ReadWrite,
}

impl RemoteCacheMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Read => "read",
            Self::ReadWrite => "read-write",
        }
    }
}

/// Remote endpoint and access mode for a cacheable execution.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteCacheConfig {
    pub mode: RemoteCacheAccess,
    /// Endpoint as configured. It is validated when the remote cache is used.
    pub url: Arc<str>,
}

/// Access permitted when remote caching is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteCacheAccess {
    Read,
    ReadWrite,
}

/// Reads a control env. An empty value counts as unset, like a CI secret that
/// isn't available to the job.
fn env_value<'a>(
    envs: &'a FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
    name: &str,
) -> Option<&'a Arc<OsStr>> {
    envs.get(EnvName::from_ref(OsStr::new(name))).filter(|value| !value.is_empty())
}

/// Resolves remote cache access for one `vp run` level from the envs visible at
/// that level. See `PlanContext::remote_cache` for what they include.
pub(crate) fn resolve(
    configured_url: Option<&Arc<str>>,
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
) -> Result<Option<RemoteCacheConfig>, Error> {
    let mode = match env_value(envs, MODE_ENV) {
        None => None,
        Some(value) => Some(match value.to_str() {
            Some("off") => RemoteCacheMode::Off,
            Some("read") => RemoteCacheMode::Read,
            Some("read-write") => RemoteCacheMode::ReadWrite,
            _ => return Err(Error::InvalidRemoteCacheModeEnv(Arc::clone(value))),
        }),
    };
    let url = match env_value(envs, URL_ENV) {
        Some(value) => Some(Arc::<str>::from(
            value.to_str().ok_or_else(|| Error::InvalidRemoteCacheUrlEnv(Arc::clone(value)))?,
        )),
        None => configured_url.filter(|url| !url.is_empty()).cloned(),
    };

    match (mode, url) {
        (Some(RemoteCacheMode::Off), _) | (None, None) => Ok(None),
        (Some(RemoteCacheMode::Read) | None, Some(url)) => {
            Ok(Some(RemoteCacheConfig { url, mode: RemoteCacheAccess::Read }))
        }
        (Some(RemoteCacheMode::ReadWrite), Some(url)) => {
            Ok(Some(RemoteCacheConfig { url, mode: RemoteCacheAccess::ReadWrite }))
        }
        (Some(RemoteCacheMode::Read | RemoteCacheMode::ReadWrite), None) => {
            Err(Error::MissingRemoteCacheEndpoint)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_envs_by_platform_name_rules() {
        let envs =
            [("vp_remote_cache", "read-write"), ("vp_remote_cache_url", "https://cache.example")]
                .into_iter()
                .map(|(name, value)| {
                    (
                        EnvName::new(Arc::<OsStr>::from(OsStr::new(name))),
                        Arc::<OsStr>::from(OsStr::new(value)),
                    )
                })
                .collect();
        let resolved = resolve(None, &envs).unwrap();
        if cfg!(windows) {
            let resolved = resolved.unwrap();
            assert_eq!(resolved.mode, RemoteCacheAccess::ReadWrite);
            assert_eq!(&*resolved.url, "https://cache.example");
        } else {
            assert!(resolved.is_none());
        }
    }
}

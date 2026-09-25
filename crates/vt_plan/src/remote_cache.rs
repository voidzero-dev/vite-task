//! Remote cache settings: the mode requested with `--remote-cache` or
//! `VP_REMOTE_CACHE`, and the access resolved from it for each `vp run` level.

use std::{ffi::OsStr, sync::Arc};

use rustc_hash::FxHashMap;
use serde::Serialize;
use vt_casefold::EnvName;

use crate::Error;

pub(crate) const MODE_ENV: &str = "VP_REMOTE_CACHE";
const URL_ENV: &str = "VP_REMOTE_CACHE_URL";

/// Remote cache mode requested with `--remote-cache` or `VP_REMOTE_CACHE`.
/// `resolve` combines it with the endpoint into a [`ResolvedRemoteCacheConfig`].
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

/// Remote cache access and endpoint resolved for a `vp run` level from the
/// requested mode, `VP_REMOTE_CACHE_URL`, and `cache.remote.url`.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedRemoteCacheConfig {
    pub access: RemoteCacheAccess,
    /// Endpoint as configured. It is validated when the remote cache is used.
    pub url: Arc<str>,
}

/// Remote cache access after resolution. `off` resolves to no remote cache.
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
/// that level. See `PlanContext::resolved_remote_cache` for what they include.
pub(crate) fn resolve(
    configured_url: Option<&Arc<str>>,
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
) -> Result<Option<ResolvedRemoteCacheConfig>, Error> {
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
            Ok(Some(ResolvedRemoteCacheConfig { access: RemoteCacheAccess::Read, url }))
        }
        (Some(RemoteCacheMode::ReadWrite), Some(url)) => {
            Ok(Some(ResolvedRemoteCacheConfig { access: RemoteCacheAccess::ReadWrite, url }))
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
            assert_eq!(resolved.access, RemoteCacheAccess::ReadWrite);
            assert_eq!(&*resolved.url, "https://cache.example");
        } else {
            assert!(resolved.is_none());
        }
    }
}

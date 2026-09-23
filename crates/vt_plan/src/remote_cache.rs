//! Remote cache settings resolved for each invocation during planning.

use std::{ffi::OsStr, str::FromStr, sync::Arc};

use rustc_hash::FxHashMap;
use serde::Serialize;
use vt_casefold::EnvName;
use vt_graph::config::user::UserRemoteCacheConfig;
use vt_str::Str;

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

impl FromStr for RemoteCacheMode {
    type Err = RemoteCacheConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "read" => Ok(Self::Read),
            "read-write" => Ok(Self::ReadWrite),
            _ => Err(RemoteCacheConfigError::InvalidMode(value.into())),
        }
    }
}

/// Remote endpoint and access mode for a cacheable execution.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteCacheConfig {
    pub mode: RemoteCacheAccess,
    /// Endpoint as configured. It is validated when the remote cache is used.
    pub url: Str,
}

/// Access permitted when remote caching is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteCacheAccess {
    Read,
    ReadWrite,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteCacheConfigError {
    #[error("Invalid remote cache mode {0:?}: expected off, read, or read-write")]
    InvalidMode(Str),
    #[error("{0} must be valid Unicode")]
    InvalidEnv(&'static str),
    #[error("Remote caching requires remoteCache.url or VP_REMOTE_CACHE_URL")]
    MissingEndpoint,
}

pub(crate) fn is_control_env<S: AsRef<OsStr> + ?Sized>(name: &EnvName<S>) -> bool {
    [MODE_ENV, URL_ENV].into_iter().any(|control| name == EnvName::from_ref(OsStr::new(control)))
}

/// Reads a control env. An empty value counts as unset, like a CI secret that
/// isn't available to the job.
fn env_value<'a>(
    envs: &'a FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
    name: &'static str,
) -> Result<Option<&'a str>, RemoteCacheConfigError> {
    envs.get(EnvName::from_ref(OsStr::new(name)))
        .filter(|value| !value.is_empty())
        .map(|value| value.to_str().ok_or(RemoteCacheConfigError::InvalidEnv(name)))
        .transpose()
}

/// Resolve against this invocation's environment, including inherited overrides.
pub(crate) fn resolve(
    configured: Option<&UserRemoteCacheConfig>,
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
) -> Result<Option<RemoteCacheConfig>, RemoteCacheConfigError> {
    let mode = env_value(envs, MODE_ENV)?.map(str::parse).transpose()?;
    let url = env_value(envs, URL_ENV)?
        .or_else(|| configured.map(|config| config.url.as_str()))
        .filter(|url| !url.is_empty())
        .map(Str::from);

    match (mode, url) {
        (Some(RemoteCacheMode::Off), _) | (None, None) => Ok(None),
        (Some(RemoteCacheMode::Read) | None, Some(url)) => {
            Ok(Some(RemoteCacheConfig { url, mode: RemoteCacheAccess::Read }))
        }
        (Some(RemoteCacheMode::ReadWrite), Some(url)) => {
            Ok(Some(RemoteCacheConfig { url, mode: RemoteCacheAccess::ReadWrite }))
        }
        (Some(RemoteCacheMode::Read | RemoteCacheMode::ReadWrite), None) => {
            Err(RemoteCacheConfigError::MissingEndpoint)
        }
    }
}

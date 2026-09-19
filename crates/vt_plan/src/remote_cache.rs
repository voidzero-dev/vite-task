//! Remote cache settings resolved for each invocation during planning.

use std::{ffi::OsStr, fmt, str::FromStr, sync::Arc};

use rustc_hash::FxHashMap;
use serde::Serialize;
use url::Url;
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

impl fmt::Display for RemoteCacheMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

/// Validated remote endpoint and access mode for a cacheable execution.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteCacheConfig {
    pub url: Arc<Url>,
    pub mode: RemoteCacheMode,
}

#[derive(Debug, thiserror::Error)]
pub enum RemoteCacheConfigError {
    #[error("Invalid remote cache mode {0:?}: expected off, read, or read-write")]
    InvalidMode(Str),
    #[error("{0} must be valid Unicode")]
    InvalidEnv(&'static str),
    #[error("Remote cache mode {0} requires remoteCache.url or VP_REMOTE_CACHE_URL")]
    MissingEndpoint(RemoteCacheMode),
    #[error("Invalid remote cache endpoint: {0}")]
    InvalidEndpoint(#[from] url::ParseError),
    #[error("Remote cache endpoint must be an HTTP or HTTPS URL with a host")]
    InvalidEndpointScheme,
}

pub(crate) fn env_name_matches(name: &OsStr, expected: &str) -> bool {
    if cfg!(windows) { name.eq_ignore_ascii_case(expected) } else { name == expected }
}

pub(crate) fn is_control_env(name: &OsStr) -> bool {
    env_name_matches(name, MODE_ENV) || env_name_matches(name, URL_ENV)
}

fn env_value<'a>(
    envs: &'a FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    name: &'static str,
) -> Result<Option<&'a str>, RemoteCacheConfigError> {
    envs.iter()
        .find(|(key, _)| env_name_matches(key, name))
        .map(|(_, value)| value.to_str().ok_or(RemoteCacheConfigError::InvalidEnv(name)))
        .transpose()
}

/// Resolve against this invocation's environment, including inherited overrides.
pub(crate) fn resolve(
    configured: Option<&UserRemoteCacheConfig>,
    envs: &FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    mode_override: Option<RemoteCacheMode>,
) -> Result<Option<RemoteCacheConfig>, RemoteCacheConfigError> {
    let mode = match mode_override {
        Some(mode) => Some(mode),
        None => env_value(envs, MODE_ENV)?.map(str::parse).transpose()?,
    };
    let endpoint =
        env_value(envs, URL_ENV)?.or_else(|| configured.map(|config| config.url.as_str()));
    let url = endpoint
        .map(|endpoint| {
            let url = Url::parse(endpoint)?;
            if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
                return Err(RemoteCacheConfigError::InvalidEndpointScheme);
            }
            Ok(Arc::new(url))
        })
        .transpose()?;

    let mode = mode.unwrap_or_else(|| {
        if url.is_some() { RemoteCacheMode::Read } else { RemoteCacheMode::Off }
    });
    if mode == RemoteCacheMode::Off {
        return Ok(None);
    }
    let url = url.ok_or(RemoteCacheConfigError::MissingEndpoint(mode))?;
    Ok(Some(RemoteCacheConfig { url, mode }))
}

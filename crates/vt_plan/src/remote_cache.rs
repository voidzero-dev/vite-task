//! Remote cache settings: the mode requested with `--remote-cache` or
//! `VP_REMOTE_CACHE`, and the access and auth resolved for each `vp run` level.

use std::{ffi::OsStr, fmt, sync::Arc};

use rustc_hash::FxHashMap;
use serde::{Serialize, Serializer};
use vt_casefold::EnvName;

use crate::Error;

pub(crate) const MODE_ENV: &str = "VP_REMOTE_CACHE";
const URL_ENV: &str = "VP_REMOTE_CACHE_URL";
/// Set in GitHub Actions jobs that can request OIDC tokens, which need
/// `permissions: id-token: write`.
const GITHUB_OIDC_REQUEST_URL_ENV: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
const GITHUB_OIDC_REQUEST_TOKEN_ENV: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

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

/// Remote cache access, endpoint, and auth resolved for a `vp run` level from
/// the requested mode, `VP_REMOTE_CACHE_URL`, `cache.remote.url`, and the envs
/// that the auth uses.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedRemoteCacheConfig {
    pub access: RemoteCacheAccess,
    /// Endpoint as configured. It is validated when the remote cache is used.
    pub url: Arc<str>,
    pub auth: RemoteCacheAuth,
}

/// How requests to the remote cache authenticate. It holds everything needed
/// to build the credentials, so nothing reads envs after planning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RemoteCacheAuth {
    /// Requests carry no credentials.
    Anonymous,
    /// Stores carry a GitHub Actions OIDC token.
    GithubOidc(GithubOidcAuth),
}

/// How stores get a GitHub Actions OIDC token: it's requested from
/// `request_url` with `request_token`, for `audience`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct GithubOidcAuth {
    pub request_url: Arc<str>,
    pub request_token: Secret,
    pub audience: Arc<str>,
}

/// A credential. Debug output and serialized plans show a placeholder
/// instead of its value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Secret(Arc<str>);

impl Secret {
    const REDACTED: &str = "<redacted>";

    #[must_use]
    pub const fn new(value: Arc<str>) -> Self {
        Self(value)
    }

    /// The value, for sending it where it's needed.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::REDACTED)
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::REDACTED)
    }
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

    let (access, url) = match (mode, url) {
        (Some(RemoteCacheMode::Off), _) | (None, None) => return Ok(None),
        (Some(RemoteCacheMode::Read) | None, Some(url)) => (RemoteCacheAccess::Read, url),
        (Some(RemoteCacheMode::ReadWrite), Some(url)) => (RemoteCacheAccess::ReadWrite, url),
        (Some(RemoteCacheMode::Read | RemoteCacheMode::ReadWrite), None) => {
            return Err(Error::MissingRemoteCacheEndpoint);
        }
    };
    let auth = resolve_auth(&url, envs)?;
    Ok(Some(ResolvedRemoteCacheConfig { access, url, auth }))
}

/// Resolves how requests to `url` authenticate from the envs visible at the
/// `vp run` level. In a GitHub Actions job that can request OIDC tokens,
/// stores carry a token whose audience is `url` without a trailing slash.
/// Otherwise requests are anonymous.
fn resolve_auth(
    url: &str,
    envs: &FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>>,
) -> Result<RemoteCacheAuth, Error> {
    let (Some(request_url), Some(request_token)) = (
        env_value(envs, GITHUB_OIDC_REQUEST_URL_ENV),
        env_value(envs, GITHUB_OIDC_REQUEST_TOKEN_ENV),
    ) else {
        return Ok(RemoteCacheAuth::Anonymous);
    };
    let utf8 = |name: &'static str, value: &OsStr| {
        value.to_str().map(Arc::from).ok_or(Error::NonUtf8RemoteCacheAuthEnv(name))
    };
    Ok(RemoteCacheAuth::GithubOidc(GithubOidcAuth {
        request_url: utf8(GITHUB_OIDC_REQUEST_URL_ENV, request_url)?,
        request_token: Secret::new(utf8(GITHUB_OIDC_REQUEST_TOKEN_ENV, request_token)?),
        audience: Arc::from(url.trim_end_matches('/')),
    }))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn envs<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a OsStr)>,
    ) -> FxHashMap<EnvName<Arc<OsStr>>, Arc<OsStr>> {
        pairs
            .into_iter()
            .map(|(name, value)| {
                (EnvName::new(Arc::<OsStr>::from(OsStr::new(name))), Arc::<OsStr>::from(value))
            })
            .collect()
    }

    #[test]
    fn reads_envs_by_platform_name_rules() {
        let envs = envs([
            ("vp_remote_cache", OsStr::new("read-write")),
            ("vp_remote_cache_url", OsStr::new("https://cache.example")),
        ]);
        let resolved = resolve(None, &envs).unwrap();
        if cfg!(windows) {
            let resolved = resolved.unwrap();
            assert_eq!(resolved.access, RemoteCacheAccess::ReadWrite);
            assert_eq!(&*resolved.url, "https://cache.example");
        } else {
            assert!(resolved.is_none());
        }
    }
    #[test]
    fn github_oidc_token_is_redacted() {
        let envs = envs([
            ("VP_REMOTE_CACHE_URL", OsStr::new("https://cache.example/projects/test/")),
            ("ACTIONS_ID_TOKEN_REQUEST_URL", OsStr::new("https://token.example/?api-version=2.0")),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", OsStr::new("request-token")),
        ]);
        let auth = resolve(None, &envs).unwrap().unwrap().auth;
        let RemoteCacheAuth::GithubOidc(github_oidc) = &auth else {
            panic!("expected GitHub OIDC, got {auth:?}");
        };
        assert_eq!(&*github_oidc.request_url, "https://token.example/?api-version=2.0");
        assert_eq!(github_oidc.request_token.expose(), "request-token");
        assert_eq!(&*github_oidc.audience, "https://cache.example/projects/test");

        let debug = vt_str::format!("{auth:?}");
        let serialized = serde_json::to_string(&auth).unwrap();
        for output in [debug.as_str(), &serialized] {
            assert!(!output.contains("request-token") && output.contains("<redacted>"), "{output}");
        }
    }

    #[test]
    fn non_utf8_auth_env_fails_without_showing_its_value() {
        #[cfg(unix)]
        let token = {
            use std::os::unix::ffi::OsStringExt as _;
            OsString::from_vec(b"secret\xff".to_vec())
        };
        #[cfg(windows)]
        let token = {
            use std::os::windows::ffi::OsStringExt as _;
            // "secret" followed by an unpaired surrogate.
            OsString::from_wide(&[0x73, 0x65, 0x63, 0x72, 0x65, 0x74, 0xD800])
        };
        let envs = envs([
            ("VP_REMOTE_CACHE_URL", OsStr::new("https://cache.example")),
            ("ACTIONS_ID_TOKEN_REQUEST_URL", OsStr::new("https://token.example")),
            ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", token.as_os_str()),
        ]);
        let error = resolve(None, &envs).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid value for ACTIONS_ID_TOKEN_REQUEST_TOKEN: not valid UTF-8"
        );
    }
}

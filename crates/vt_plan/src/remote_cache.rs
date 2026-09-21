//! Remote cache settings carried by planned executions.

use std::sync::Arc;

use serde::Serialize;
use url::Url;

/// Remote access requested by an invocation. Local cache policy is independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteCacheMode {
    Off,
    Read,
    ReadWrite,
}

/// Remote endpoint and access mode for a cacheable execution.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteCacheConfig {
    pub url: Arc<Url>,
    pub mode: RemoteCacheMode,
}

//! Credentials for remote cache requests. An [`Auth`] supplies the headers
//! that each request carries.

use std::{error::Error as StdError, fmt, pin::Pin, sync::Arc};

use reqwest::header::HeaderMap;

mod github_oidc;

pub use github_oidc::GithubOidc;

/// The operation a request performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// `POST {endpoint}/fetch`.
    Fetch,
    /// `GET {endpoint}/blob/{blob_id}`.
    Download,
    /// `POST {endpoint}/store`.
    Store,
}

/// Why an [`Auth`] couldn't supply credentials.
pub type AuthError = Arc<dyn StdError + Send + Sync>;

/// The headers that an [`Auth`] supplies for a request, once they're ready.
pub type AuthHeaders<'a> = Pin<Box<dyn Future<Output = Result<HeaderMap, AuthError>> + Send + 'a>>;

/// Supplies the headers that authenticate requests to an endpoint.
pub trait Auth: fmt::Debug + Send + Sync {
    /// The headers to add to a request for `operation`. The request is sent
    /// once they're ready, and isn't sent if this fails. `http` is the
    /// client's HTTP client, for any requests needed to get the credentials.
    /// It doesn't follow redirects.
    fn headers<'a>(&'a self, operation: Operation, http: &'a reqwest::Client) -> AuthHeaders<'a>;
}

/// Requests carry no credentials.
#[derive(Debug)]
pub struct Anonymous;

impl Auth for Anonymous {
    fn headers<'a>(&'a self, _: Operation, _: &'a reqwest::Client) -> AuthHeaders<'a> {
        Box::pin(std::future::ready(Ok(HeaderMap::new())))
    }
}

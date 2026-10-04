//! Authenticates stores with GitHub Actions OIDC tokens.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{
    StatusCode,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use tokio::sync::Mutex;
use url::{ParseError, Url};
use vt_str::Str;

use super::{Auth, AuthError, AuthHeaders, Operation};

/// Time allowed for a token request, from sending it until its whole response
/// arrives.
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A token that expires within this time isn't reused. The server checks that
/// the token is still valid when it publishes a store, and gives a store at
/// most two minutes, so a store that starts with a reused token ends before
/// the token expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(120);

/// Authenticates stores with a GitHub Actions OIDC token, sent as
/// `Authorization: Bearer <token>`. Fetches and downloads carry no
/// credentials.
///
/// The token is requested when the first store needs it, and later stores
/// reuse it until shortly before it expires. Concurrent stores wait for the
/// same request. Once a request fails, every later store fails the same way,
/// without another request.
pub struct GithubOidc {
    /// How to request a token, or why the settings can't make a request.
    request: Result<TokenRequest, AuthError>,
    state: Mutex<TokenState>,
}

impl GithubOidc {
    /// Authenticate stores with tokens for `audience`, requested from
    /// `request_url` with `request_token`. In a GitHub Actions job, those are
    /// the values of `ACTIONS_ID_TOKEN_REQUEST_URL` and
    /// `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. Nothing is requested yet. If
    /// `request_url` isn't an HTTP or HTTPS URL, or `request_token` can't be
    /// sent in a header, every store fails.
    #[must_use]
    pub fn new(request_url: &str, request_token: &str, audience: &str) -> Self {
        let request = TokenRequest::new(request_url, request_token, audience)
            .map_err(|err| -> AuthError { Arc::new(err) });
        Self { request, state: Mutex::new(TokenState::Empty) }
    }

    /// The `Authorization` header for a store.
    async fn authorization(&self, http: &reqwest::Client) -> Result<HeaderValue, AuthError> {
        let request = self.request.as_ref().map_err(Arc::clone)?;
        // The lock is held during the request, so concurrent stores wait for
        // its token instead of making their own requests.
        let mut state = self.state.lock().await;
        match &*state {
            TokenState::Failed(err) => return Err(Arc::clone(err)),
            TokenState::Cached { authorization, refresh_at } if SystemTime::now() < *refresh_at => {
                return Ok(authorization.clone());
            }
            TokenState::Empty | TokenState::Cached { .. } => {}
        }
        let sent = request.send(http).await.map_err(|err| -> AuthError { Arc::new(err) });
        *state = match &sent {
            Ok(token) => token.reuse(),
            Err(err) => TokenState::Failed(Arc::clone(err)),
        };
        drop(state);
        sent.map(|token| token.authorization)
    }
}

impl fmt::Debug for GithubOidc {
    /// Leaves out the tokens.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let url = self.request.as_ref().ok().map(|request| request.url.as_str());
        f.debug_struct("GithubOidc").field("url", &url).finish_non_exhaustive()
    }
}

impl Auth for GithubOidc {
    fn headers<'a>(&'a self, operation: Operation, http: &'a reqwest::Client) -> AuthHeaders<'a> {
        Box::pin(async move {
            let mut headers = HeaderMap::new();
            if operation == Operation::Store {
                headers.insert(AUTHORIZATION, self.authorization(http).await?);
            }
            Ok(headers)
        })
    }
}

/// The token a store can reuse, if any.
enum TokenState {
    /// No token can be reused.
    Empty,
    /// The `Authorization` header with a token to reuse until `refresh_at`.
    Cached { authorization: HeaderValue, refresh_at: SystemTime },
    /// The last request failed, so every store fails with its error.
    Failed(AuthError),
}

/// Why no token could be obtained. The messages name only the kind of
/// failure, and none of them contain a token.
#[derive(Debug, thiserror::Error)]
enum TokenError {
    /// The source is the parse error if the request URL isn't a URL at all.
    #[error("invalid GitHub Actions OIDC token request URL")]
    InvalidRequestUrl(#[source] Option<ParseError>),
    #[error("invalid GitHub Actions OIDC request token")]
    InvalidRequestToken,
    /// No complete response arrived. The source leaves out the request URL.
    #[error("GitHub Actions OIDC token request failed")]
    Network(#[source] reqwest::Error),
    #[error("GitHub Actions OIDC token request failed with HTTP status {}", .0.as_u16())]
    Status(StatusCode),
    /// The body isn't JSON with a token in `value`, or the token can't be
    /// sent in a header.
    #[error("malformed GitHub Actions OIDC token response")]
    MalformedResponse,
}

/// How to request a token.
struct TokenRequest {
    /// The request URL, with the audience added to its query.
    url: Url,
    /// `Bearer <request token>`.
    authorization: HeaderValue,
}

/// A token from a successful request.
struct Token {
    /// `Bearer <token>`.
    authorization: HeaderValue,
    /// When the token expires, if it says.
    expiry: Option<SystemTime>,
}

impl Token {
    /// The state that lets later stores reuse this token until shortly
    /// before it expires.
    fn reuse(&self) -> TokenState {
        self.expiry.and_then(|expiry| expiry.checked_sub(REFRESH_MARGIN)).map_or(
            TokenState::Empty,
            |refresh_at| TokenState::Cached {
                authorization: self.authorization.clone(),
                refresh_at,
            },
        )
    }
}

/// The body of a successful token response.
#[derive(Deserialize)]
struct TokenResponse {
    value: Str,
}

impl TokenRequest {
    fn new(request_url: &str, request_token: &str, audience: &str) -> Result<Self, TokenError> {
        let mut url =
            Url::parse(request_url).map_err(|err| TokenError::InvalidRequestUrl(Some(err)))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(TokenError::InvalidRequestUrl(None));
        }
        url.query_pairs_mut().append_pair("audience", audience);
        let authorization = bearer(request_token).ok_or(TokenError::InvalidRequestToken)?;
        Ok(Self { url, authorization })
    }

    async fn send(&self, http: &reqwest::Client) -> Result<Token, TokenError> {
        let network_error = |err: reqwest::Error| TokenError::Network(err.without_url());
        let response = http
            .get(self.url.clone())
            .header(AUTHORIZATION, self.authorization.clone())
            .header(ACCEPT, "application/json")
            .timeout(TOKEN_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if status != StatusCode::OK {
            return Err(TokenError::Status(status));
        }
        let body = response.bytes().await.map_err(network_error)?;
        let TokenResponse { value } =
            serde_json::from_slice(&body).map_err(|_| TokenError::MalformedResponse)?;
        if value.is_empty() {
            return Err(TokenError::MalformedResponse);
        }
        let authorization = bearer(&value).ok_or(TokenError::MalformedResponse)?;
        Ok(Token { authorization, expiry: expiry(&value) })
    }
}

/// `Bearer <token>`, marked sensitive so that debug output leaves it out, or
/// `None` if `token` can't be sent in a header.
fn bearer(token: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(&vt_str::format!("Bearer {token}")).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// When `token`, a JSON Web Token, expires according to its `exp` claim. The
/// claim isn't verified, since it only decides when to request a new token.
fn expiry(token: &str) -> Option<SystemTime> {
    #[derive(Deserialize)]
    struct Claims {
        exp: u64,
    }

    let payload = token.split('.').nth(1)?;
    let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let Claims { exp } = serde_json::from_slice(&payload).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(exp))
}

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, thread::JoinHandle};

    use super::*;
    use crate::{
        Client, Error,
        test_server::{contains, no_request_waiting, serve_once},
    };

    const AUDIENCE: &str = "https://cache.example/projects/test";

    /// A JSON Web Token that expires `lifetime` from now. Only its payload is
    /// read; `id` tells tokens apart.
    fn jwt(lifetime: Duration, id: &str) -> Str {
        let exp = (SystemTime::now() + lifetime).duration_since(SystemTime::UNIX_EPOCH).unwrap();
        let payload =
            URL_SAFE_NO_PAD.encode(vt_str::format!("{{\"exp\":{}}}", exp.as_secs()).as_bytes());
        vt_str::format!("eyJhbGciOiJSUzI1NiJ9.{payload}.{id}")
    }

    fn token_response(token: &str) -> Vec<u8> {
        vt_str::format!("{{\"count\":1,\"value\":\"{token}\"}}").as_bytes().to_vec()
    }

    /// A token endpoint and a cache endpoint on loopback, and a client for
    /// the cache endpoint whose stores get tokens from the token endpoint.
    struct Endpoints {
        token: Arc<TcpListener>,
        cache: Arc<TcpListener>,
        auth: Arc<GithubOidc>,
        client: Client,
    }

    impl Endpoints {
        fn new() -> Self {
            let token = TcpListener::bind("127.0.0.1:0").unwrap();
            let cache = TcpListener::bind("127.0.0.1:0").unwrap();
            let request_url =
                vt_str::format!("http://{}/token?api-version=2.0", token.local_addr().unwrap());
            let auth = Arc::new(GithubOidc::new(&request_url, "request-token", AUDIENCE));
            let endpoint = vt_str::format!("http://{}/projects/test", cache.local_addr().unwrap());
            let client = Client::new(&endpoint, Arc::clone(&auth) as Arc<dyn Auth>).unwrap();
            Self { token: Arc::new(token), cache: Arc::new(cache), auth, client }
        }

        /// Serve token requests in turn, responding with `responses`.
        fn serve_tokens(
            &self,
            responses: Vec<(&'static str, Vec<u8>)>,
        ) -> JoinHandle<Vec<Vec<u8>>> {
            serve(&self.token, responses)
        }

        /// Serve `count` stores in turn, each responding with 200.
        fn serve_stores(&self, count: usize) -> JoinHandle<Vec<Vec<u8>>> {
            serve(&self.cache, vec![("HTTP/1.1 200 OK", Vec::new()); count])
        }

        async fn store(&self) -> Result<(), Error> {
            self.client.store(b"k", b"s", b"v", None).await
        }
    }

    /// Serve a request for each of `responses` in turn, and return the
    /// requests.
    fn serve(
        listener: &Arc<TcpListener>,
        responses: Vec<(&'static str, Vec<u8>)>,
    ) -> JoinHandle<Vec<Vec<u8>>> {
        let listener = Arc::clone(listener);
        std::thread::spawn(move || {
            responses.iter().map(|(status, body)| serve_once(&listener, status, body)).collect()
        })
    }

    /// The error's message, followed by its sources' messages.
    fn messages(error: &Error) -> Vec<Str> {
        std::iter::successors(Some(error as &dyn std::error::Error), |err| err.source())
            .map(|err| vt_str::format!("{err}"))
            .collect()
    }

    #[tokio::test]
    async fn store_carries_a_token_for_the_audience() {
        let endpoints = Endpoints::new();
        let token = jwt(Duration::from_secs(300), "a");
        let tokens = endpoints.serve_tokens(vec![("HTTP/1.1 200 OK", token_response(&token))]);
        let stores = endpoints.serve_stores(1);

        endpoints.store().await.unwrap();

        let token_request = &tokens.join().unwrap()[0];
        let target = "GET /token?api-version=2.0&audience=https%3A%2F%2Fcache.example%2Fprojects%2Ftest HTTP/1.1\r\n";
        assert!(token_request.starts_with(target.as_bytes()));
        assert!(contains(token_request, b"authorization: Bearer request-token\r\n"));
        assert!(contains(token_request, b"accept: application/json\r\n"));
        let store_request = &stores.join().unwrap()[0];
        let authorization = vt_str::format!("authorization: Bearer {token}\r\n");
        assert!(contains(store_request, authorization.as_bytes()));

        let debug = vt_str::format!("{:?}", endpoints.auth);
        assert!(!debug.contains("request-token") && !debug.contains(token.as_str()), "{debug}");
    }

    #[tokio::test]
    async fn fetches_and_downloads_carry_no_token() {
        let endpoints = Endpoints::new();
        let cache = Arc::clone(&endpoints.cache);
        let server = std::thread::spawn(move || {
            [
                serve_once(&cache, "HTTP/1.1 404 Not Found", b""),
                serve_once(&cache, "HTTP/1.1 404 Not Found", b""),
            ]
        });

        assert_eq!(endpoints.client.fetch(b"k", b"s").await.unwrap(), None);
        endpoints.client.download("7").await.unwrap_err();

        for request in server.join().unwrap() {
            assert!(!contains(&request, b"authorization:"));
        }
        assert!(no_request_waiting(&endpoints.token));
    }

    #[tokio::test]
    async fn stores_reuse_a_token_until_shortly_before_it_expires() {
        let endpoints = Endpoints::new();
        let token = jwt(Duration::from_secs(300), "a");
        let tokens = endpoints.serve_tokens(vec![("HTTP/1.1 200 OK", token_response(&token))]);
        let stores = endpoints.serve_stores(2);

        endpoints.store().await.unwrap();
        endpoints.store().await.unwrap();

        tokens.join().unwrap();
        assert!(no_request_waiting(&endpoints.token));
        let authorization = vt_str::format!("authorization: Bearer {token}\r\n");
        for request in stores.join().unwrap() {
            assert!(contains(&request, authorization.as_bytes()));
        }
    }

    #[tokio::test]
    async fn concurrent_stores_wait_for_one_token_request() {
        let endpoints = Endpoints::new();
        let token = jwt(Duration::from_secs(300), "a");
        let tokens = endpoints.serve_tokens(vec![("HTTP/1.1 200 OK", token_response(&token))]);
        let stores = endpoints.serve_stores(2);

        let (first, second) = tokio::join!(endpoints.store(), endpoints.store());
        first.unwrap();
        second.unwrap();

        tokens.join().unwrap();
        assert!(no_request_waiting(&endpoints.token));
        stores.join().unwrap();
    }

    #[tokio::test]
    async fn token_that_expires_soon_or_does_not_say_is_not_reused() {
        for first in [jwt(Duration::from_secs(100), "a"), Str::from("opaque")] {
            let endpoints = Endpoints::new();
            let second = jwt(Duration::from_secs(300), "b");
            let tokens = endpoints.serve_tokens(vec![
                ("HTTP/1.1 200 OK", token_response(&first)),
                ("HTTP/1.1 200 OK", token_response(&second)),
            ]);
            let stores = endpoints.serve_stores(2);

            endpoints.store().await.unwrap();
            endpoints.store().await.unwrap();

            assert_eq!(tokens.join().unwrap().len(), 2);
            let stores = stores.join().unwrap();
            for (request, token) in stores.iter().zip([&first, &second]) {
                let authorization = vt_str::format!("authorization: Bearer {token}\r\n");
                assert!(contains(request, authorization.as_bytes()), "{first}");
            }
        }
    }

    #[tokio::test]
    async fn failed_token_request_fails_later_stores_without_requests() {
        let endpoints = Endpoints::new();
        let tokens = endpoints.serve_tokens(vec![("HTTP/1.1 403 Forbidden", b"denied".to_vec())]);

        let first = endpoints.store().await.unwrap_err();
        tokens.join().unwrap();
        let second = endpoints.store().await.unwrap_err();

        for error in [first, second] {
            assert!(matches!(error, Error::Auth(_)), "{error:?}");
            assert_eq!(
                messages(&error),
                [
                    "failed to authenticate",
                    "GitHub Actions OIDC token request failed with HTTP status 403"
                ]
            );
        }
        assert!(no_request_waiting(&endpoints.token));
        assert!(no_request_waiting(&endpoints.cache));
    }

    #[tokio::test]
    async fn malformed_token_responses_fail_the_store() {
        for body in [
            b"not json".as_slice(),
            b"{}",
            br#"{"value":1}"#,
            br#"{"value":""}"#,
            // A token with a newline can't be sent in a header.
            br#"{"value":"a\nb"}"#,
        ] {
            let endpoints = Endpoints::new();
            let tokens = endpoints.serve_tokens(vec![("HTTP/1.1 200 OK", body.to_vec())]);

            let error = endpoints.store().await.unwrap_err();
            assert_eq!(
                messages(&error),
                ["failed to authenticate", "malformed GitHub Actions OIDC token response"]
            );
            tokens.join().unwrap();
            assert!(no_request_waiting(&endpoints.cache));
        }
    }

    #[tokio::test]
    async fn unreachable_token_endpoint_is_a_network_error() {
        let cache = TcpListener::bind("127.0.0.1:0").unwrap();
        // Nothing can listen on port 0.
        let auth = GithubOidc::new("http://127.0.0.1:0/token", "request-token", AUDIENCE);
        let endpoint = vt_str::format!("http://{}/projects/test", cache.local_addr().unwrap());
        let client = Client::new(&endpoint, Arc::new(auth)).unwrap();

        let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
        let messages = messages(&error);
        assert_eq!(
            messages[..2],
            ["failed to authenticate", "GitHub Actions OIDC token request failed"]
        );
        for message in &messages {
            assert!(
                !message.contains("127.0.0.1:0/token") && !message.contains("request-token"),
                "{message}"
            );
        }
        assert!(no_request_waiting(&cache));
    }

    #[tokio::test]
    async fn invalid_settings_fail_stores_but_not_fetches() {
        for (request_url, request_token, message) in [
            ("not a url", "request-token", "invalid GitHub Actions OIDC token request URL"),
            (
                "ftp://token.example/",
                "request-token",
                "invalid GitHub Actions OIDC token request URL",
            ),
            ("http://token.example/", "a\nb", "invalid GitHub Actions OIDC request token"),
        ] {
            let cache = Arc::new(TcpListener::bind("127.0.0.1:0").unwrap());
            let auth = GithubOidc::new(request_url, request_token, AUDIENCE);
            let endpoint = vt_str::format!("http://{}/projects/test", cache.local_addr().unwrap());
            let client = Client::new(&endpoint, Arc::new(auth)).unwrap();

            let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert_eq!(messages(&error)[..2], ["failed to authenticate", message]);
            assert!(no_request_waiting(&cache));

            let server = serve(&cache, vec![("HTTP/1.1 404 Not Found", Vec::new())]);
            assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
            server.join().unwrap();
        }
    }
}

//! Client for the remote cache server API. Keys, values, and blobs are opaque
//! bytes; the caller decides what they contain.

use std::{
    fmt,
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use bytes::Bytes;
use reqwest::{
    Response, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize};
use url::{ParseError, Url};
use vt_path::AbsolutePath;
use vt_str::Str;

/// Time allowed to establish a connection, including the TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for each read of a response. Until the response headers
/// arrive, it also bounds sending the request.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A cached OIDC token is replaced once it expires within this long, so it
/// doesn't expire while a store is being sent.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// A failed remote cache operation. The messages name only the kind of
/// failure, so they are the same on every platform. The details, if any, are
/// in the source.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint isn't an HTTP or HTTPS URL that can have a path. The
    /// source is the parse error if it isn't a URL at all.
    #[error("invalid endpoint")]
    InvalidEndpoint(#[source] Option<ParseError>),
    /// The HTTP client couldn't be created, for example because no root
    /// certificates could be loaded.
    #[error("failed to create the HTTP client")]
    HttpClient(#[source] reqwest::Error),
    /// The blob file couldn't be opened.
    #[error("failed to read the blob")]
    ReadBlob(#[source] std::io::Error),
    /// No complete response arrived, for example because the connection
    /// failed, timed out, or closed before the whole body arrived. The source
    /// leaves out the request URL, because the endpoint may contain
    /// credentials, such as a token in its query.
    #[error("network error")]
    Network(#[source] reqwest::Error),
    /// The server responded with a status other than 200, or for a fetch,
    /// other than 200 or 404. The source is the message in the response body,
    /// if any.
    #[error("HTTP status {}", .0.as_u16())]
    Status(StatusCode, #[source] Option<ServerMessage>),
    /// The body of a 200 fetch response isn't an exact or fallback match.
    #[error("malformed response")]
    MalformedResponse(#[source] ciborium::de::Error<std::io::Error>),
    /// No GitHub Actions OIDC token could be obtained for a store. Later
    /// stores return the same error without requesting another token.
    #[error("failed to get a GitHub Actions OIDC token")]
    OidcToken(#[source] Arc<OidcTokenError>),
    /// The server responded to a store with 401 from a GitHub Actions job
    /// that can't request OIDC tokens, so the store had none. The message is
    /// the same as for any other 401 response.
    #[error("HTTP status 401")]
    MissingIdTokenPermission(#[source] IdTokenPermissionHint),
}

impl Error {
    /// Whether the error means the store wasn't authorized: no OIDC token
    /// could be obtained, or the server responded with 401 or 403.
    #[must_use]
    pub const fn is_unauthorized(&self) -> bool {
        matches!(
            self,
            Self::OidcToken(_)
                | Self::MissingIdTokenPermission(_)
                | Self::Status(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _)
        )
    }
}

/// The message in the body of an error response.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ServerMessage(Str);

/// Why no GitHub Actions OIDC token could be obtained. Neither the messages
/// nor the sources contain the token endpoint's URL, the request token, or
/// the response body.
#[derive(Debug, thiserror::Error)]
pub enum OidcTokenError {
    /// The request URL isn't a URL, or the request token can't be sent in a
    /// header.
    #[error("invalid token request")]
    InvalidRequest,
    /// No complete response arrived.
    #[error("network error")]
    Network(#[source] reqwest::Error),
    /// The token endpoint responded with a status other than 200. Redirects
    /// aren't followed, so a redirect is a status like any other.
    #[error("HTTP status {}", .0.as_u16())]
    Status(StatusCode),
    /// The body of the 200 response isn't a JSON object whose `value` is a
    /// JWT with an integer `exp` claim.
    #[error("malformed response")]
    MalformedResponse,
}

/// The cause of a 401 response to a store sent without a token from a GitHub
/// Actions job: the job can't request OIDC tokens. The source is the message
/// in the response body, if any.
#[derive(Debug, thiserror::Error)]
#[error("grant `id-token: write` to this job")]
pub struct IdTokenPermissionHint(#[source] Option<ServerMessage>);

/// How [`Client::store`] authenticates. Fetches and downloads never send
/// credentials.
#[derive(Debug, Clone, Default)]
pub enum StoreAuth {
    /// Send no credentials.
    #[default]
    Anonymous,
    /// Send a GitHub Actions OIDC token as `Authorization: Bearer <token>`.
    GithubOidc(GithubOidc),
    /// Send no credentials from a GitHub Actions job that can't request OIDC
    /// tokens. A 401 response is [`Error::MissingIdTokenPermission`].
    GithubActionsWithoutOidc,
}

/// What a GitHub Actions job uses to request OIDC tokens.
///
/// These are the values of `ACTIONS_ID_TOKEN_REQUEST_URL` and
/// `ACTIONS_ID_TOKEN_REQUEST_TOKEN`. GitHub sets them only in jobs granted
/// `id-token: write`. `Debug` leaves both out.
#[derive(Clone)]
pub struct GithubOidc {
    request_url: Arc<str>,
    request_token: Arc<str>,
}

impl GithubOidc {
    #[must_use]
    pub fn new(request_url: &str, request_token: &str) -> Self {
        Self { request_url: Arc::from(request_url), request_token: Arc::from(request_token) }
    }
}

impl fmt::Debug for GithubOidc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GithubOidc").finish_non_exhaustive()
    }
}

/// The OIDC token that stores share. A failed request is kept, so later
/// stores get its error without another request.
enum TokenState {
    Empty,
    /// `authorization` is `Bearer <token>`, and `expires_at` is the token's
    /// `exp` claim, in seconds since the Unix epoch.
    Valid {
        authorization: HeaderValue,
        expires_at: u64,
    },
    Failed(Arc<OidcTokenError>),
}

/// The response of the token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    value: Box<str>,
}

/// The only claim read from a token.
#[derive(Deserialize)]
struct TokenClaims {
    exp: u64,
}

/// The audience of tokens for `endpoint`: the endpoint without its query,
/// fragment, userinfo, or trailing slash.
fn audience(endpoint: &Url) -> Str {
    let mut url = endpoint.clone();
    url.set_query(None);
    url.set_fragment(None);
    // These fail only for URLs that can't have userinfo, which have none.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let url = url.as_str();
    Str::from(url.strip_suffix('/').unwrap_or(url))
}

/// The `exp` claim in the payload of `jwt`, read without verifying the
/// signature.
fn jwt_expiry(jwt: &str) -> Option<u64> {
    let payload = URL_SAFE_NO_PAD.decode(jwt.split('.').nth(1)?).ok()?;
    let TokenClaims { exp } = serde_json::from_slice(&payload).ok()?;
    Some(exp)
}

/// Whether a token that expires at `expires_at`, in seconds since the Unix
/// epoch, expires within [`TOKEN_REFRESH_MARGIN`].
fn expires_soon(expires_at: u64) -> bool {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs());
    expires_at <= now.saturating_add(TOKEN_REFRESH_MARGIN.as_secs())
}

/// The `Authorization` header value `Bearer <token>`, marked sensitive so
/// it's left out of debug output. `None` if `token` can't be sent in a header.
fn bearer(token: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_bytes(&[b"Bearer ", token.as_bytes()].concat()).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// A blob being downloaded.
#[derive(Debug)]
pub struct Download {
    response: Response,
}

impl Download {
    /// The next chunk of the blob, or `None` after the last one.
    ///
    /// # Errors
    ///
    /// Returns an error if the rest of the blob can't be read, for example
    /// because the connection closes before it all arrives.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, Error> {
        self.response.chunk().await.map_err(network_error)
    }
}

/// The `metadata` part of a store request.
#[derive(Serialize)]
struct StoreMetadata<'a> {
    #[serde(with = "serde_bytes")]
    key: &'a [u8],
    #[serde(with = "serde_bytes")]
    secondary_key: &'a [u8],
    #[serde(with = "serde_bytes")]
    value: &'a [u8],
}

/// The keys a fetch looks up.
#[derive(Serialize)]
struct FetchRequest<'a> {
    #[serde(with = "serde_bytes")]
    key: &'a [u8],
    #[serde(with = "serde_bytes")]
    secondary_key: &'a [u8],
}

/// The body of a 200 fetch response: an exact or fallback match.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fetched {
    /// The entry stored under the requested key.
    Exact {
        /// The stored value.
        #[serde(with = "serde_bytes")]
        value: Vec<u8>,
        /// The ID to download the entry's blob with, if it has one. The field
        /// must be present, with null for no blob.
        #[serde(deserialize_with = "Option::deserialize")]
        blob_id: Option<Str>,
    },
    /// No entry is stored under the requested key, but the secondary key is
    /// associated with the entry stored under `key`. The fallback entry's
    /// value and blob ID aren't decoded.
    Fallback {
        /// The key the fallback entry is stored under.
        #[serde(with = "serde_bytes")]
        key: Vec<u8>,
    },
}

/// A client for one remote cache endpoint.
pub struct Client {
    http: reqwest::Client,
    fetch_url: Url,
    store_url: Url,
    /// `{endpoint}/blob`, to which each download appends a blob ID.
    blob_url: Url,
    store_auth: StoreAuth,
    /// The audience OIDC tokens are requested for. See [`audience`].
    audience: Str,
    oidc_token: tokio::sync::Mutex<TokenState>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Leave out the cached token and the endpoint, which may contain
        // credentials.
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Create a client for `endpoint`, a base URL that may include a
    /// namespace path, such as `https://cache.example.com/projects/my-project`.
    /// Stores authenticate with `store_auth`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEndpoint`] if `endpoint` isn't a usable URL, or
    /// [`Error::HttpClient`] if the HTTP client can't be created.
    pub fn new(endpoint: &str, store_auth: StoreAuth) -> Result<Self, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let fetch_url = route_url(&endpoint, "fetch")?;
        let store_url = route_url(&endpoint, "store")?;
        let blob_url = route_url(&endpoint, "blob")?;
        // reqwest configures TLS with the process's default crypto provider.
        // Installing fails if one is already installed; vite-plus installs
        // ring too.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A redirect fails like any other status. Following one could turn a
        // store into a GET of a login page that responds with 200.
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Error::HttpClient)?;
        Ok(Self {
            http,
            fetch_url,
            store_url,
            blob_url,
            store_auth,
            audience: audience(&endpoint),
            oidc_token: tokio::sync::Mutex::new(TokenState::Empty),
        })
    }

    /// Fetch the entry stored under `key` with `POST {endpoint}/fetch`,
    /// falling back to the entry associated with `secondary_key`. Returns
    /// `None` for a 404 response, which means neither key matched.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the server responds with a
    /// status other than 200 or 404, or the body of a 200 response isn't an
    /// exact or fallback match.
    pub async fn fetch(&self, key: &[u8], secondary_key: &[u8]) -> Result<Option<Fetched>, Error> {
        let body = encode_cbor(&FetchRequest { key, secondary_key });
        let response = self
            .http
            .post(self.fetch_url.clone())
            .header(CONTENT_TYPE, "application/cbor")
            .body(body)
            .send()
            .await
            .map_err(network_error)?;
        if response.status() == StatusCode::NOT_FOUND {
            // Read the body so the connection can be reused. It doesn't matter
            // if that fails, because the status alone is the answer.
            let _ = response.bytes().await;
            return Ok(None);
        }
        let body = check_status(response).await?.bytes().await.map_err(network_error)?;
        decode_fetched(&body).map(Some)
    }

    /// Start downloading the blob `blob_id` with
    /// `GET {endpoint}/blob/{blob_id}`. Its chunks are read from the returned
    /// [`Download`] as they arrive.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the server responds with a
    /// status other than 200.
    pub async fn download(&self, blob_id: &str) -> Result<Download, Error> {
        let mut url = self.blob_url.clone();
        url.path_segments_mut().map_err(|()| Error::InvalidEndpoint(None))?.push(blob_id);
        let response = self.http.get(url).send().await.map_err(network_error)?;
        Ok(Download { response: check_status(response).await? })
    }

    /// Store `value` under `key` with `POST {endpoint}/store`, uploading the
    /// file at `blob` as its blob. Fetches that match no key fall back to this
    /// entry through `secondary_key`. The request authenticates as the client's
    /// [`StoreAuth`] says, first requesting an OIDC token if it needs one.
    ///
    /// # Errors
    ///
    /// Returns an error if no OIDC token can be obtained, the blob file can't
    /// be opened, the request fails, or the server responds with a status
    /// other than 200.
    pub async fn store(
        &self,
        key: &[u8],
        secondary_key: &[u8],
        value: &[u8],
        blob: Option<&AbsolutePath>,
    ) -> Result<(), Error> {
        let authorization = match &self.store_auth {
            StoreAuth::GithubOidc(oidc) => {
                Some(self.oidc_authorization(oidc).await.map_err(Error::OidcToken)?)
            }
            StoreAuth::Anonymous | StoreAuth::GithubActionsWithoutOidc => None,
        };
        let metadata = StoreMetadata { key, secondary_key, value };
        let mut form = Form::new().part("metadata", metadata_part(&metadata));
        if let Some(blob) = blob {
            form = form.part("blob", blob_part(blob).await?);
        }
        let mut request = self.http.post(self.store_url.clone()).multipart(form);
        if let Some(authorization) = authorization {
            // `headers` replaces the basic credentials taken from userinfo in
            // the endpoint, so only the token is sent.
            request = request.headers(HeaderMap::from_iter([(AUTHORIZATION, authorization)]));
        }
        let response = request.send().await.map_err(network_error)?;
        let response = match check_status(response).await {
            Err(Error::Status(StatusCode::UNAUTHORIZED, message))
                if matches!(self.store_auth, StoreAuth::GithubActionsWithoutOidc) =>
            {
                return Err(Error::MissingIdTokenPermission(IdTokenPermissionHint(message)));
            }
            response => response?,
        };
        // The response's blob ID isn't needed. Read the body anyway, so the
        // connection can be reused.
        response.bytes().await.map_err(network_error)?;
        Ok(())
    }

    /// The `Authorization` header value for a store: the cached token, or a
    /// new one if it's missing or about to expire. The lock is held during
    /// the request, so stores that need a token at the same time share it. A
    /// new token is used even if it's about to expire itself; the server
    /// decides whether it's valid.
    async fn oidc_authorization(
        &self,
        oidc: &GithubOidc,
    ) -> Result<HeaderValue, Arc<OidcTokenError>> {
        let mut state = self.oidc_token.lock().await;
        match &*state {
            TokenState::Valid { authorization, expires_at } if !expires_soon(*expires_at) => {
                return Ok(authorization.clone());
            }
            TokenState::Failed(err) => return Err(Arc::clone(err)),
            TokenState::Valid { .. } | TokenState::Empty => {}
        }
        let requested = self.request_oidc_token(oidc).await.map_err(Arc::new);
        *state = match &requested {
            Ok((authorization, expires_at)) => {
                TokenState::Valid { authorization: authorization.clone(), expires_at: *expires_at }
            }
            Err(err) => TokenState::Failed(Arc::clone(err)),
        };
        requested.map(|(authorization, _)| authorization)
    }

    /// Request a token with `GET <request URL>&audience=<audience>`. Returns
    /// its `Authorization` header value and its `exp` claim.
    async fn request_oidc_token(
        &self,
        oidc: &GithubOidc,
    ) -> Result<(HeaderValue, u64), OidcTokenError> {
        let mut url = Url::parse(&oidc.request_url).map_err(|_| OidcTokenError::InvalidRequest)?;
        url.query_pairs_mut().append_pair("audience", &self.audience);
        let request_authorization =
            bearer(&oidc.request_token).ok_or(OidcTokenError::InvalidRequest)?;
        let response = self
            .http
            .get(url)
            .header(AUTHORIZATION, request_authorization)
            .send()
            .await
            .map_err(|err| OidcTokenError::Network(err.without_url()))?;
        if response.status() != StatusCode::OK {
            return Err(OidcTokenError::Status(response.status()));
        }
        let body =
            response.bytes().await.map_err(|err| OidcTokenError::Network(err.without_url()))?;
        // The parse error isn't kept, because it can quote the body.
        let TokenResponse { value } =
            serde_json::from_slice(&body).map_err(|_| OidcTokenError::MalformedResponse)?;
        let expires_at = jwt_expiry(&value).ok_or(OidcTokenError::MalformedResponse)?;
        let authorization = bearer(&value).ok_or(OidcTokenError::MalformedResponse)?;
        Ok((authorization, expires_at))
    }
}

/// Only HTTP 200 counts as success. For other statuses, the error includes the
/// message in the response body.
async fn check_status(response: Response) -> Result<Response, Error> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(response);
    }
    let message = response.text().await.ok().and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| ServerMessage(Str::from(text)))
    });
    Err(Error::Status(status, message))
}

fn network_error(err: reqwest::Error) -> Error {
    Error::Network(err.without_url())
}

fn decode_fetched(body: &[u8]) -> Result<Fetched, Error> {
    ciborium::from_reader(body).map_err(Error::MalformedResponse)
}

fn parse_endpoint(endpoint: &str) -> Result<Url, Error> {
    let url = Url::parse(endpoint).map_err(|err| Error::InvalidEndpoint(Some(err)))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidEndpoint(None));
    }
    Ok(url)
}

/// Append `route` to the endpoint's path, keeping its namespace path.
fn route_url(endpoint: &Url, route: &str) -> Result<Url, Error> {
    let mut url = endpoint.clone();
    url.path_segments_mut().map_err(|()| Error::InvalidEndpoint(None))?.pop_if_empty().push(route);
    Ok(url)
}

fn encode_cbor(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).expect("encoding byte strings into a Vec can't fail");
    bytes
}

fn metadata_part(metadata: &StoreMetadata<'_>) -> Part {
    Part::bytes(encode_cbor(metadata)).mime_str("application/cbor").expect("valid MIME type")
}

async fn blob_part(path: &AbsolutePath) -> Result<Part, Error> {
    let file = tokio::fs::File::open(path).await.map_err(Error::ReadBlob)?;
    let length = file.metadata().await.map_err(Error::ReadBlob)?.len();
    Ok(Part::stream_with_length(file, length)
        .mime_str("application/octet-stream")
        .expect("valid MIME type"))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
    };

    use vt_path::AbsolutePathBuf;

    use super::*;

    fn store_url(endpoint: &str) -> Result<Url, Error> {
        route_url(&parse_endpoint(endpoint)?, "store")
    }

    #[test]
    fn store_url_keeps_the_namespace_path() {
        for (endpoint, expected) in [
            ("http://cache.example/projects/test", "http://cache.example/projects/test/store"),
            ("http://cache.example/projects/test/", "http://cache.example/projects/test/store"),
            ("https://cache.example", "https://cache.example/store"),
            ("https://cache.example/ns?token=a", "https://cache.example/ns/store?token=a"),
        ] {
            assert_eq!(store_url(endpoint).unwrap().as_str(), expected, "{endpoint}");
        }
    }

    #[test]
    fn rejects_endpoints_that_are_not_http_urls() {
        for endpoint in ["cache.example/projects/test", "ftp://cache.example", "mailto:a@b.example"]
        {
            assert!(
                matches!(
                    Client::new(endpoint, StoreAuth::Anonymous),
                    Err(Error::InvalidEndpoint(_))
                ),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn audience_is_the_endpoint_without_query_userinfo_or_trailing_slash() {
        for (endpoint, expected) in [
            ("https://cache.example/projects/test", "https://cache.example/projects/test"),
            ("https://cache.example/projects/test/", "https://cache.example/projects/test"),
            ("https://cache.example", "https://cache.example"),
            ("https://cache.example/ns?token=a#top", "https://cache.example/ns"),
            ("https://user:password@cache.example:8443/ns", "https://cache.example:8443/ns"),
            ("HTTPS://Cache.Example:443/ns", "https://cache.example/ns"),
        ] {
            assert_eq!(
                audience(&parse_endpoint(endpoint).unwrap()).as_str(),
                expected,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn metadata_is_a_cbor_map_of_byte_strings() {
        let metadata = StoreMetadata { key: b"k", secondary_key: b"", value: &[0x00, 0xff] };
        let mut expected = vec![0xa3];
        expected.extend(b"\x63key\x41k");
        expected.extend(b"\x6dsecondary_key\x40");
        expected.extend(b"\x65value\x42\x00\xff");
        assert_eq!(encode_cbor(&metadata), expected);
    }

    fn cbor_map(fields: Vec<(&str, ciborium::Value)>) -> Vec<u8> {
        let map = fields.into_iter().map(|(name, value)| (name.into(), value)).collect();
        encode_cbor(&ciborium::Value::Map(map))
    }

    #[test]
    fn decodes_each_kind_of_fetch_response() {
        let bytes = |bytes: &[u8]| ciborium::Value::Bytes(bytes.to_vec());
        let exact = cbor_map(vec![
            ("kind", "exact".into()),
            ("value", bytes(b"\x00value")),
            ("blob_id", "1".into()),
        ]);
        assert_eq!(
            decode_fetched(&exact).unwrap(),
            Fetched::Exact { value: b"\x00value".to_vec(), blob_id: Some(Str::from("1")) }
        );

        let without_blob = cbor_map(vec![
            ("kind", "exact".into()),
            ("value", bytes(b"value")),
            ("blob_id", ciborium::Value::Null),
        ]);
        assert_eq!(
            decode_fetched(&without_blob).unwrap(),
            Fetched::Exact { value: b"value".to_vec(), blob_id: None }
        );

        let fallback = cbor_map(vec![
            ("kind", "fallback".into()),
            ("key", bytes(b"stored key")),
            ("value", bytes(b"value")),
            ("blob_id", "2".into()),
        ]);
        assert_eq!(
            decode_fetched(&fallback).unwrap(),
            Fetched::Fallback { key: b"stored key".to_vec() }
        );
    }

    #[test]
    fn rejects_malformed_fetch_responses() {
        for body in [
            b"\xffnot cbor".to_vec(),
            cbor_map(vec![("kind", "unknown".into())]),
            // A miss is a 404 response, not a kind.
            cbor_map(vec![("kind", "not_found".into())]),
            // The value must be a byte string.
            cbor_map(vec![
                ("kind", "exact".into()),
                ("value", 1.into()),
                ("blob_id", ciborium::Value::Null),
            ]),
            // `blob_id` is nullable, but it must be present.
            cbor_map(vec![
                ("kind", "exact".into()),
                ("value", ciborium::Value::Bytes(b"value".to_vec())),
            ]),
        ] {
            let error = decode_fetched(&body).unwrap_err();
            assert!(matches!(error, Error::MalformedResponse(_)), "{error:?}");
            assert_eq!(error.to_string(), "malformed response");
        }
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    /// A response with `status_line` and `body` that closes the connection,
    /// so the client opens a new one for its next request.
    fn http_response(status_line: &str, body: &[u8]) -> Vec<u8> {
        let head = vt_str::format!(
            "{status_line}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        [head.as_bytes(), body].concat()
    }

    /// Accept one HTTP request, respond with `status_line` and `body`, and
    /// return the raw request.
    fn serve_once(listener: &TcpListener, status_line: &str, body: &[u8]) -> Vec<u8> {
        serve_raw_once(listener, &http_response(status_line, body))
    }

    /// Accept one request for each of `responses`, write them in order, and
    /// return the raw requests.
    fn serve_each(
        listener: TcpListener,
        responses: Vec<Vec<u8>>,
    ) -> std::thread::JoinHandle<Vec<Vec<u8>>> {
        std::thread::spawn(move || {
            responses.iter().map(|response| serve_raw_once(&listener, response)).collect()
        })
    }

    /// Accept one HTTP request, write `response`, close the connection, and
    /// return the raw request.
    fn serve_raw_once(listener: &TcpListener, response: &[u8]) -> Vec<u8> {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        let header_end = loop {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request headers ended");
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let content_length: usize = std::str::from_utf8(&request[..header_end])
            .unwrap()
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request body ended");
            request.extend_from_slice(&buf[..n]);
        }
        stream.write_all(response).unwrap();
        request
    }

    fn client_for(listener: &TcpListener) -> Client {
        client_with_auth(listener, StoreAuth::Anonymous)
    }

    fn client_with_auth(listener: &TcpListener, store_auth: StoreAuth) -> Client {
        let port = listener.local_addr().unwrap().port();
        Client::new(&vt_str::format!("http://127.0.0.1:{port}/projects/test"), store_auth).unwrap()
    }

    const REQUEST_TOKEN: &str = "request-token-secret";

    /// A client whose token endpoint is `/token` on the same server.
    fn oidc_client_for(listener: &TcpListener) -> Client {
        let port = listener.local_addr().unwrap().port();
        let request_url = vt_str::format!("http://127.0.0.1:{port}/token?api-version=2.0");
        client_with_auth(
            listener,
            StoreAuth::GithubOidc(GithubOidc::new(&request_url, REQUEST_TOKEN)),
        )
    }

    fn unix_now() -> u64 {
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
    }

    /// A JWT with a fake signature that expires at `exp`, in seconds since
    /// the Unix epoch.
    fn jwt(exp: u64) -> Str {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(vt_str::format!(r#"{{"exp":{exp}}}"#).as_str());
        vt_str::format!("{header}.{payload}.c2lnbmF0dXJl")
    }

    fn token_response(token: &str) -> Vec<u8> {
        http_response("HTTP/1.1 200 OK", vt_str::format!(r#"{{"value":"{token}"}}"#).as_bytes())
    }

    fn stored() -> Vec<u8> {
        http_response("HTTP/1.1 200 OK", b"")
    }

    fn authorization_line(token: &str) -> Vec<u8> {
        vt_str::format!("authorization: Bearer {token}\r\n").as_bytes().to_vec()
    }

    fn is_token_request(request: &[u8]) -> bool {
        request.starts_with(b"GET /token?")
    }

    fn is_store_request(request: &[u8]) -> bool {
        request.starts_with(b"POST /projects/test/store HTTP/1.1\r\n")
    }

    /// The message of `error` and each of its sources, and its debug output.
    fn error_texts(error: &Error) -> Vec<Str> {
        std::iter::successors(Some(error as &dyn std::error::Error), |err| err.source())
            .map(|err| vt_str::format!("{err}"))
            .chain([vt_str::format!("{error:?}")])
            .collect()
    }

    /// The `metadata` part of a store request for key `k`, secondary key `s`,
    /// and value `v`.
    fn metadata_part_bytes() -> Vec<u8> {
        let metadata = StoreMetadata { key: b"k", secondary_key: b"s", value: b"v" };
        [
            b"name=\"metadata\"\r\nContent-Type: application/cbor\r\n\r\n".as_slice(),
            &encode_cbor(&metadata),
            b"\r\n",
        ]
        .concat()
    }

    #[tokio::test]
    async fn fetch_posts_the_keys_as_cbor() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let body = cbor_map(vec![
            ("kind", "exact".into()),
            ("value", ciborium::Value::Bytes(b"v".to_vec())),
            ("blob_id", ciborium::Value::Null),
        ]);
        let server = std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", &body));

        assert_eq!(
            client.fetch(b"k", b"s").await.unwrap(),
            Some(Fetched::Exact { value: b"v".to_vec(), blob_id: None })
        );

        let request = server.join().unwrap();
        assert!(request.starts_with(b"POST /projects/test/fetch HTTP/1.1\r\n"));
        assert!(contains(&request, b"content-type: application/cbor\r\n"));
        let keys = encode_cbor(&FetchRequest { key: b"k", secondary_key: b"s" });
        assert!(request.ends_with(&[b"\r\n\r\n".as_slice(), &keys].concat()));
    }

    #[tokio::test]
    async fn fetch_of_a_missing_entry_is_none() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"Not found")
        });

        assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn fetch_ignores_an_incomplete_404_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        // The connection closes before the announced length arrives.
        let server = std::thread::spawn(move || {
            serve_raw_once(&listener, b"HTTP/1.1 404 Not Found\r\ncontent-length: 100\r\n\r\nNot")
        });

        assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn fetch_fails_on_an_error_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"try later")
        });

        let error = client.fetch(b"k", b"s").await.unwrap_err();
        assert!(matches!(error, Error::Status(StatusCode::SERVICE_UNAVAILABLE, _)), "{error:?}");
        assert_eq!(std::error::Error::source(&error).unwrap().to_string(), "try later");
        server.join().unwrap();
    }

    async fn read_to_end(mut download: Download) -> Result<Vec<u8>, Error> {
        let mut blob = Vec::new();
        while let Some(chunk) = download.chunk().await? {
            blob.extend_from_slice(&chunk);
        }
        Ok(blob)
    }

    #[tokio::test]
    async fn download_streams_the_blob() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server =
            std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", b"archive bytes"));

        let download = client.download("7").await.unwrap();
        assert_eq!(read_to_end(download).await.unwrap(), b"archive bytes");

        let request = server.join().unwrap();
        assert!(request.starts_with(b"GET /projects/test/blob/7 HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn download_of_a_missing_blob_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"Blob not found")
        });

        let error = client.download("7").await.unwrap_err();
        assert_eq!(error.to_string(), "HTTP status 404");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn incomplete_download_is_a_network_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        // The connection closes before the announced length arrives.
        let server = std::thread::spawn(move || {
            serve_raw_once(&listener, b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\npartial")
        });

        let download = client.download("7").await.unwrap();
        let error = read_to_end(download).await.unwrap_err();
        assert!(matches!(error, Error::Network(_)), "{error:?}");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn store_posts_metadata_and_blob_parts() {
        let dir = tempfile::tempdir().unwrap();
        let blob = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        std::fs::write(blob.as_path(), b"archive bytes").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 500 Internal Server Error", b"storage failed\n")
        });

        let err = client.store(b"k", b"s", b"v", Some(&blob)).await.unwrap_err();
        assert!(matches!(err, Error::Status(StatusCode::INTERNAL_SERVER_ERROR, _)));
        assert_eq!(err.to_string(), "HTTP status 500");
        assert_eq!(std::error::Error::source(&err).unwrap().to_string(), "storage failed");

        let request = server.join().unwrap();
        assert!(request.starts_with(b"POST /projects/test/store HTTP/1.1\r\n"));
        assert!(contains(&request, b"content-type: multipart/form-data; boundary="));
        assert!(contains(&request, &metadata_part_bytes()));
        let blob =
            b"name=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\narchive bytes\r\n";
        assert!(contains(&request, blob));
    }

    #[tokio::test]
    async fn store_without_a_blob_sends_only_metadata() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        // `0xff` can't start a CBOR item, so this body doesn't decode.
        let server =
            std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", b"\xffnot cbor"));

        client.store(b"k", b"s", b"v", None).await.unwrap();

        let request = server.join().unwrap();
        assert!(request.starts_with(b"POST /projects/test/store HTTP/1.1\r\n"));
        assert!(contains(&request, &metadata_part_bytes()));
        assert!(!contains(&request, b"name=\"blob\""));
        assert!(!contains(&request, b"authorization:"));
    }

    #[tokio::test]
    async fn store_does_not_follow_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_raw_once(
                &listener,
                b"HTTP/1.1 302 Found\r\nlocation: /login\r\ncontent-length: 0\r\n\r\n",
            )
        });

        let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
        assert!(matches!(error, Error::Status(StatusCode::FOUND, None)), "{error:?}");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn network_errors_leave_out_the_url() {
        let client = Client::new(
            "http://user:password@127.0.0.1:0/projects/test?token=secret",
            StoreAuth::Anonymous,
        )
        .unwrap();

        let error = client.fetch(b"k", b"s").await.unwrap_err();
        assert!(matches!(error, Error::Network(_)), "{error:?}");
        for text in error_texts(&error) {
            assert!(!text.contains("password") && !text.contains("secret"), "{text}");
        }
    }

    #[tokio::test]
    async fn only_stores_send_an_oidc_token() {
        let dir = tempfile::tempdir().unwrap();
        let blob = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        std::fs::write(blob.as_path(), b"archive bytes").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = oidc_client_for(&listener);
        let token = jwt(unix_now() + 3600);
        let server = serve_each(
            listener,
            vec![
                http_response("HTTP/1.1 404 Not Found", b""),
                stored(),
                token_response(&token),
                stored(),
            ],
        );

        assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
        read_to_end(client.download("7").await.unwrap()).await.unwrap();
        client.store(b"k", b"s", b"v", Some(&blob)).await.unwrap();

        let [fetch_request, download_request, token_request, store_request] =
            server.join().unwrap().try_into().unwrap();
        assert!(!contains(&fetch_request, b"authorization:"));
        assert!(!contains(&download_request, b"authorization:"));
        let token_request_line = vt_str::format!(
            "GET /token?api-version=2.0&audience=http%3A%2F%2F127.0.0.1%3A{port}%2Fprojects%2Ftest HTTP/1.1\r\n"
        );
        assert!(token_request.starts_with(token_request_line.as_bytes()));
        assert!(contains(&token_request, &authorization_line(REQUEST_TOKEN)));
        assert!(is_store_request(&store_request));
        assert!(contains(&store_request, &authorization_line(&token)));
        assert!(!contains(&store_request, REQUEST_TOKEN.as_bytes()));
        assert!(contains(&store_request, b"archive bytes"));

        let store_auth = StoreAuth::GithubOidc(GithubOidc::new("https://token.example/", "secret"));
        for text in [vt_str::format!("{client:?}"), vt_str::format!("{store_auth:?}")] {
            for secret in [token.as_str(), REQUEST_TOKEN, "secret", "token.example", "127.0.0.1"] {
                assert!(!text.contains(secret), "{text}");
            }
        }
    }

    #[tokio::test]
    async fn oidc_token_is_reused_until_it_expires_soon() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = oidc_client_for(&listener);
        // The first token expires within the refresh margin.
        let expiring = jwt(unix_now() + 30);
        let fresh = jwt(unix_now() + 3600);
        let server = serve_each(
            listener,
            vec![token_response(&expiring), stored(), token_response(&fresh), stored(), stored()],
        );

        for _ in 0..3 {
            client.store(b"k", b"s", b"v", None).await.unwrap();
        }

        let requests = server.join().unwrap();
        assert!(is_token_request(&requests[0]));
        assert!(contains(&requests[1], &authorization_line(&expiring)));
        assert!(is_token_request(&requests[2]));
        assert!(contains(&requests[3], &authorization_line(&fresh)));
        assert!(contains(&requests[4], &authorization_line(&fresh)));
    }

    #[tokio::test]
    async fn concurrent_stores_share_one_token_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = oidc_client_for(&listener);
        let token = jwt(unix_now() + 3600);
        // A second token request would get a store response, which isn't a
        // token.
        let server = serve_each(listener, vec![token_response(&token), stored(), stored()]);

        let (first, second) = tokio::join!(
            client.store(b"k", b"s", b"v", None),
            client.store(b"k", b"s", b"v", None),
        );
        first.unwrap();
        second.unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests.iter().filter(|request| is_token_request(request)).count(), 1);
        for request in requests.iter().filter(|request| is_store_request(request)) {
            assert!(contains(request, &authorization_line(&token)));
        }
    }

    #[tokio::test]
    async fn failed_token_request_sends_no_store() {
        let token_in_a_string = vt_str::format!(r#""{}""#, jwt(unix_now() + 3600));
        let payload = |json: &str| URL_SAFE_NO_PAD.encode(json);
        let without_exp = vt_str::format!(r#"{{"value":"h.{}.s"}}"#, payload(r#"{"iat":1}"#));
        // The JSON escapes decode to a line break, which can't be in a header.
        let with_line_break =
            vt_str::format!(r#"{{"value":"h.{}.s\r\nx: y"}}"#, payload(r#"{"exp":4102444800}"#));
        for (response, expected) in [
            (
                http_response("HTTP/1.1 500 Internal Server Error", b"secret body"),
                "HTTP status 500",
            ),
            (
                b"HTTP/1.1 302 Found\r\nlocation: /projects/test/store\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    .to_vec(),
                "HTTP status 302",
            ),
            (http_response("HTTP/1.1 200 OK", b"secret body"), "malformed response"),
            (http_response("HTTP/1.1 200 OK", token_in_a_string.as_bytes()), "malformed response"),
            (http_response("HTTP/1.1 200 OK", br#"{"token":"secret"}"#), "malformed response"),
            (http_response("HTTP/1.1 200 OK", br#"{"value":"secret"}"#), "malformed response"),
            (http_response("HTTP/1.1 200 OK", without_exp.as_bytes()), "malformed response"),
            (http_response("HTTP/1.1 200 OK", with_line_break.as_bytes()), "malformed response"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let client = oidc_client_for(&listener);
            // Only the token request is served. A store would fail to connect.
            let server = serve_each(listener, vec![response]);

            let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert!(matches!(error, Error::OidcToken(_)), "{error:?}");
            assert!(error.is_unauthorized());
            assert_eq!(error.to_string(), "failed to get a GitHub Actions OIDC token");
            assert_eq!(std::error::Error::source(&error).unwrap().to_string(), expected);
            for text in error_texts(&error) {
                // `eyJ` starts each part of the token, encoding `{"`.
                for secret in ["secret", "eyJ", "/token", "127.0.0.1", REQUEST_TOKEN] {
                    assert!(!text.contains(secret), "{text}");
                }
            }
            assert!(is_token_request(&server.join().unwrap()[0]));

            // The failure is kept, so no other token request is made.
            let again = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert_eq!(error_texts(&again), error_texts(&error));
        }
    }

    #[tokio::test]
    async fn invalid_token_request_settings_send_no_request() {
        for oidc in [
            GithubOidc::new("not a url", REQUEST_TOKEN),
            GithubOidc::new("http://127.0.0.1:0/token", "secret\r\nx-injected: 1"),
        ] {
            // Nothing can listen on port 0, so a request would be a network
            // error.
            let client =
                Client::new("http://127.0.0.1:0/projects/test", StoreAuth::GithubOidc(oidc))
                    .unwrap();
            let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert_eq!(
                error_texts(&error)[..2],
                ["failed to get a GitHub Actions OIDC token", "invalid token request"]
            );
        }
    }

    #[tokio::test]
    async fn only_a_401_from_github_actions_without_oidc_has_a_hint() {
        for (store_auth, status_line, expected) in [
            (
                StoreAuth::GithubActionsWithoutOidc,
                "HTTP/1.1 401 Unauthorized",
                ["HTTP status 401", "grant `id-token: write` to this job", "denied"].as_slice(),
            ),
            (
                StoreAuth::GithubActionsWithoutOidc,
                "HTTP/1.1 403 Forbidden",
                &["HTTP status 403", "denied"],
            ),
            (StoreAuth::Anonymous, "HTTP/1.1 401 Unauthorized", &["HTTP status 401", "denied"]),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let client = client_with_auth(&listener, store_auth);
            let server = std::thread::spawn(move || serve_once(&listener, status_line, b"denied"));

            let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert!(error.is_unauthorized());
            assert_eq!(error_texts(&error)[..expected.len()], *expected);
            assert!(!contains(&server.join().unwrap(), b"authorization:"));
        }
    }
}

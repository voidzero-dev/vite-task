//! Client for the remote cache server API. Keys, values, and blobs are opaque
//! bytes; the caller decides what they contain.

pub mod auth;
mod retry;
#[cfg(test)]
mod test_server;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use reqwest::{
    Body, Response, StatusCode,
    header::{CONTENT_TYPE, HeaderMap},
    multipart::{Form, Part},
};
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::Semaphore;
use url::{ParseError, Url};
use vt_path::AbsolutePath;
use vt_str::Str;

use crate::{
    auth::{Auth, AuthError, Operation},
    retry::{AttemptError, OutageBreaker, RateLimit, RetryConfig, parse_retry_after},
};

/// Time allowed to establish a connection, including the TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for each read of a response. Until the response headers
/// arrive, it also bounds sending the request.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Total timeout for a single `POST /fetch` attempt. Fetches transfer small
/// metadata payloads, so bounding them more tightly lets a hung lookup retry
/// or fall back quickly instead of stalling a task for [`READ_TIMEOUT`].
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Idle timeout for pooled HTTP/1.1 keep-alive connections, kept below typical
/// NAT and edge proxy idle cutoffs to reduce stale-socket resets on reuse.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of concurrent `POST /store` uploads in flight per [`Client`].
///
/// Bounding uploads per client shapes bursts against backends with per-isolate
/// admission limits, prevents uplink thrashing, and caps open blob file
/// handles.
pub const MAX_CONCURRENT_STORES: usize = 4;

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
    /// The request's credentials couldn't be obtained, so it wasn't sent.
    /// The source says why.
    #[error("failed to authenticate")]
    Auth(#[source] AuthError),
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
    /// The remote cache endpoint recently failed repeatedly, so this request
    /// was skipped during the outage cooldown window.
    #[error("remote cache temporarily unavailable")]
    Unavailable,
}

/// The message in the body of an error response.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ServerMessage(Str);

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

/// One of the entries stored under a key.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct Entry {
    /// The stored value.
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
    /// The ID to download the entry's blob with, if it has one. The field
    /// must be present, with null for no blob.
    #[serde(deserialize_with = "Option::deserialize")]
    pub blob_id: Option<Str>,
}

/// The body of a 200 fetch response: an exact or fallback match.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fetched {
    /// The entries stored under the requested key, newest first.
    Exact {
        /// At least one entry. An empty list is a malformed response.
        #[serde(deserialize_with = "non_empty")]
        entries: Vec<Entry>,
    },
    /// No entry is stored under the requested key, but the secondary key is
    /// associated with the entries stored under `key`. Their values and blob
    /// IDs aren't decoded.
    Fallback {
        /// The key the fallback entry is stored under.
        #[serde(with = "serde_bytes")]
        key: Vec<u8>,
    },
}

/// A client for one remote cache endpoint.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    auth: Arc<dyn Auth>,
    fetch_url: Url,
    store_url: Url,
    /// `{endpoint}/blob`, to which each download appends a blob ID.
    blob_url: Url,
    /// Bounds concurrent `POST /store` uploads in flight for this endpoint.
    store_permits: Semaphore,
    /// Shared cooldown for `429 Too Many Requests` on read operations so
    /// upload rate limits do not stall fetches on the task critical path.
    read_rate_limit: RateLimit,
    /// Shared cooldown for `429 Too Many Requests` on store operations.
    store_rate_limit: RateLimit,
    /// Circuit breaker that short-circuits requests during sustained outages.
    outage: OutageBreaker,
    retry: RetryConfig,
}

impl Client {
    /// Create a client for `endpoint`, a base URL that may include a
    /// namespace path, such as `https://cache.example.com/projects/my-project`.
    /// Each request carries the headers that `auth` supplies for it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEndpoint`] if `endpoint` isn't a usable URL, or
    /// [`Error::HttpClient`] if the HTTP client can't be created.
    pub fn new(endpoint: &str, auth: Arc<dyn Auth>) -> Result<Self, Error> {
        Self::with_retry(endpoint, auth, RetryConfig::DEFAULT)
    }

    fn with_retry(endpoint: &str, auth: Arc<dyn Auth>, retry: RetryConfig) -> Result<Self, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let fetch_url = route_url(&endpoint, "fetch")?;
        let store_url = route_url(&endpoint, "store")?;
        let blob_url = route_url(&endpoint, "blob")?;
        // reqwest configures TLS with the process's default crypto provider.
        // Installing fails if one is already installed; vite-plus installs
        // ring too.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A redirect fails like any other status. Following one could turn a
        // store into a GET of a login page that responds with 200. Force
        // HTTP/1.1 connection pooling (even if another crate enables reqwest's
        // `http2` feature) so concurrent uploads and fetches get independent
        // sockets instead of sharing fate on one multiplexed H2 connection.
        // reqwest's default 15s TCP keep-alive stays enabled.
        let http = reqwest::Client::builder()
            .http1_only()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .pool_idle_timeout(POOL_IDLE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(Error::HttpClient)?;
        Ok(Self {
            http,
            auth,
            fetch_url,
            store_url,
            blob_url,
            store_permits: Semaphore::new(MAX_CONCURRENT_STORES),
            read_rate_limit: RateLimit::default(),
            store_rate_limit: RateLimit::default(),
            outage: OutageBreaker::new(retry.outage_threshold, retry.outage_cooldown),
            retry,
        })
    }

    async fn auth_headers(&self, operation: Operation) -> Result<HeaderMap, Error> {
        self.auth.headers(operation, &self.http).await.map_err(Error::Auth)
    }

    /// Fetch the entries stored under `key` with `POST {endpoint}/fetch`,
    /// falling back to the key associated with `secondary_key`. Returns
    /// `None` for a 404 response, which means neither key matched. Transient
    /// network errors and retryable HTTP statuses (`408`, `429`, `5xx` except
    /// `501`/`505`/`511`) are retried with jittered exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns an error if the credentials can't be obtained, the request
    /// fails, the server responds with a status other than 200 or 404, or the
    /// body of a 200 response isn't an exact or fallback match.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn fetch(&self, key: &[u8], secondary_key: &[u8]) -> Result<Option<Fetched>, Error> {
        let body = Bytes::from(encode_cbor(&FetchRequest { key, secondary_key }));
        let policy = self.retry.read;
        let result = retry::run("fetch", policy, &self.read_rate_limit, &self.outage, || {
            let body = body.clone();
            async move {
                let start = Instant::now();
                let headers = self.auth_headers(Operation::Fetch).await?;
                let response = self
                    .http
                    .post(self.fetch_url.clone())
                    .headers(headers)
                    .header(CONTENT_TYPE, "application/cbor")
                    .timeout(FETCH_TIMEOUT)
                    .body(body)
                    .send()
                    .await
                    .map_err(network_error)?;
                log_response(&response, start);
                if response.status() == StatusCode::NOT_FOUND {
                    // Read the body so the connection can be reused. It doesn't matter
                    // if that fails, because the status alone is the answer.
                    let _ = response.bytes().await;
                    return Ok(None);
                }
                let body = check_status(response, policy.max_retry_after)
                    .await?
                    .bytes()
                    .await
                    .map_err(network_error)?;
                tracing::debug!(
                    bytes = body.len(),
                    elapsed = ?start.elapsed(),
                    "remote cache fetch received"
                );
                decode_fetched(&body).map(Some).map_err(AttemptError::from)
            }
        })
        .await;
        drop(body);
        result
    }

    /// Start downloading the blob `blob_id` with
    /// `GET {endpoint}/blob/{blob_id}`. Its chunks are read from the returned
    /// [`Download`] as they arrive. Transient network errors and retryable
    /// HTTP statuses (`408`, `429`, `5xx` except `501`/`505`/`511`) before the
    /// response headers arrive are retried with jittered exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns an error if the credentials can't be obtained, the request
    /// fails, or the server responds with a status other than 200.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn download(&self, blob_id: &str) -> Result<Download, Error> {
        self.download_with_policy(blob_id, self.retry.read).await
    }

    /// Start downloading the blob `blob_id` with a single attempt (no header
    /// retries), used when retrying a download that dropped mid-stream so
    /// outer and inner retry layers do not multiply.
    ///
    /// # Errors
    ///
    /// Returns an error if the credentials can't be obtained, the request
    /// fails, or the server responds with a status other than 200.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn download_once(&self, blob_id: &str) -> Result<Download, Error> {
        self.download_with_policy(blob_id, self.retry.read.once()).await
    }

    async fn download_with_policy(
        &self,
        blob_id: &str,
        policy: retry::RetryPolicy,
    ) -> Result<Download, Error> {
        let mut url = self.blob_url.clone();
        url.path_segments_mut().map_err(|()| Error::InvalidEndpoint(None))?.push(blob_id);
        retry::run("download", policy, &self.read_rate_limit, &self.outage, || {
            let url = url.clone();
            async move {
                let start = Instant::now();
                let headers = self.auth_headers(Operation::Download).await?;
                let response =
                    self.http.get(url).headers(headers).send().await.map_err(network_error)?;
                log_response(&response, start);
                Ok(Download { response: check_status(response, policy.max_retry_after).await? })
            }
        })
        .await
    }

    /// Store `value` under `key` with `POST {endpoint}/store`, uploading the
    /// file at `blob` as its blob. Fetches that match no key fall back to this
    /// entry through `secondary_key`. At most [`MAX_CONCURRENT_STORES`] stores
    /// run concurrently per client, and transient network errors or retryable
    /// HTTP statuses (`408`, `429`, `5xx` except `501`/`505`/`511`) are
    /// retried with jittered exponential backoff.
    ///
    /// # Errors
    ///
    /// Returns an error if the credentials can't be obtained, the blob file
    /// can't be opened, the request fails, or the server responds with a
    /// status other than 200.
    #[tracing::instrument(level = "debug", skip_all)]
    #[expect(clippy::missing_panics_doc, reason = "store_permits semaphore is never closed")]
    pub async fn store(
        &self,
        key: &[u8],
        secondary_key: &[u8],
        value: &[u8],
        blob: Option<&AbsolutePath>,
    ) -> Result<(), Error> {
        let queue_start = Instant::now();
        let permit = self.store_permits.acquire().await.expect("store semaphore is never closed");
        let queued = queue_start.elapsed();
        let metadata_bytes = Bytes::from(encode_cbor(&StoreMetadata { key, secondary_key, value }));
        let value_bytes = value.len();
        let policy = self.retry.store;
        let result = retry::run("store", policy, &self.store_rate_limit, &self.outage, || {
            let metadata_bytes = metadata_bytes.clone();
            async move {
                let start = Instant::now();
                let headers = self.auth_headers(Operation::Store).await?;
                let auth = start.elapsed();
                let mut form = Form::new().part("metadata", metadata_part(metadata_bytes));
                let blob_bytes = if let Some(blob) = blob {
                    let (part, length) = blob_part(blob).await?;
                    form = form.part("blob", part);
                    Some(length)
                } else {
                    None
                };
                tracing::debug!(
                    value_bytes,
                    blob_bytes,
                    ?queued,
                    ?auth,
                    "sending a remote cache store"
                );
                let response = self
                    .http
                    .post(self.store_url.clone())
                    .headers(headers)
                    .multipart(form)
                    .send()
                    .await
                    .map_err(network_error)?;
                log_response(&response, start);
                // The response's blob ID isn't needed. Read the body anyway, so the
                // connection can be reused.
                check_status(response, policy.max_retry_after)
                    .await?
                    .bytes()
                    .await
                    .map_err(network_error)?;
                Ok(())
            }
        })
        .await;
        drop(permit);
        result
    }
}

/// Log the status and length of `response`, and the time since `start`, when
/// the operation started. The request URL is left out, since the endpoint may
/// contain credentials.
fn log_response(response: &Response, start: Instant) {
    tracing::debug!(
        status = response.status().as_u16(),
        content_length = response.content_length(),
        elapsed = ?start.elapsed(),
        "remote cache responded"
    );
}

/// Only HTTP 200 counts as success. For other statuses, the error includes the
/// message in the response body. For `429 Too Many Requests`, any valid
/// integer `Retry-After` header is also returned (clamped to `max_retry_after`)
/// so the client's shared cooldown can honor it.
async fn check_status(
    response: Response,
    max_retry_after: Duration,
) -> Result<Response, AttemptError> {
    let status = response.status();
    if status == StatusCode::OK {
        return Ok(response);
    }
    let rate_limit_delay = (status == StatusCode::TOO_MANY_REQUESTS)
        .then(|| parse_retry_after(response.headers(), max_retry_after))
        .flatten();
    let message = response.text().await.ok().and_then(|text| {
        let text = text.trim();
        (!text.is_empty()).then(|| ServerMessage(Str::from(text)))
    });
    Err(AttemptError { error: Error::Status(status, message), rate_limit_delay })
}

fn network_error(err: reqwest::Error) -> Error {
    Error::Network(err.without_url())
}

fn non_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Entry>, D::Error> {
    let entries = Vec::<Entry>::deserialize(deserializer)?;
    if entries.is_empty() {
        return Err(serde::de::Error::invalid_length(0, &"at least one entry"));
    }
    Ok(entries)
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

fn metadata_part(metadata_bytes: Bytes) -> Part {
    let length = u64::try_from(metadata_bytes.len()).expect("metadata length fits in u64");
    Part::stream_with_length(Body::from(metadata_bytes), length)
        .mime_str("application/cbor")
        .expect("valid MIME type")
}

/// The blob part streamed from the file at `path`, and its length.
async fn blob_part(path: &AbsolutePath) -> Result<(Part, u64), Error> {
    let file = tokio::fs::File::open(path).await.map_err(Error::ReadBlob)?;
    let length = file.metadata().await.map_err(Error::ReadBlob)?.len();
    let part = Part::stream_with_length(file, length)
        .mime_str("application/octet-stream")
        .expect("valid MIME type");
    Ok((part, length))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use reqwest::header::{HeaderName, HeaderValue};
    use vt_path::AbsolutePathBuf;

    use super::*;
    use crate::{
        auth::{Anonymous, AuthHeaders},
        retry::RetryPolicy,
        test_server::{contains, no_request_waiting, serve_once, serve_raw_once},
    };

    /// Zero-delay retry config with the same attempt counts as production, so
    /// retry tests exercise the full retry loop without sleeping.
    const FAST_RETRY: RetryConfig = RetryConfig {
        read: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            ..RetryPolicy::READ
        },
        store: RetryPolicy {
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            ..RetryPolicy::STORE
        },
        ..RetryConfig::DEFAULT
    };

    /// Single-attempt retry config for tests that intentionally drop the
    /// connection after inspecting one request (such as TLS `ClientHello`).
    const NO_RETRY: RetryConfig = RetryConfig {
        read: RetryPolicy { max_retries: 0, ..FAST_RETRY.read },
        store: RetryPolicy { max_retries: 0, ..FAST_RETRY.store },
        ..FAST_RETRY
    };

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
            let client = Client::new(endpoint, Arc::new(Anonymous));
            assert!(matches!(client, Err(Error::InvalidEndpoint(_))), "{endpoint}");
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

    fn cbor_value(fields: Vec<(&str, ciborium::Value)>) -> ciborium::Value {
        ciborium::Value::Map(fields.into_iter().map(|(name, value)| (name.into(), value)).collect())
    }

    /// A fetch response with an exact match of `entries`.
    fn exact(entries: Vec<ciborium::Value>) -> Vec<u8> {
        cbor_map(vec![("kind", "exact".into()), ("entries", ciborium::Value::Array(entries))])
    }

    #[test]
    fn decodes_each_kind_of_fetch_response() {
        let bytes = |bytes: &[u8]| ciborium::Value::Bytes(bytes.to_vec());
        let entries = exact(vec![
            cbor_value(vec![("value", bytes(b"\x00newest")), ("blob_id", "2".into())]),
            cbor_value(vec![("blob_id", ciborium::Value::Null), ("value", bytes(b"older"))]),
        ]);
        assert_eq!(
            decode_fetched(&entries).unwrap(),
            Fetched::Exact {
                entries: vec![
                    Entry { value: b"\x00newest".to_vec(), blob_id: Some(Str::from("2")) },
                    Entry { value: b"older".to_vec(), blob_id: None },
                ]
            }
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
            // An exact match has at least one entry.
            exact(vec![]),
            // The entries must be a list, not the single entry that earlier servers sent.
            cbor_map(vec![
                ("kind", "exact".into()),
                ("value", ciborium::Value::Bytes(b"value".to_vec())),
                ("blob_id", ciborium::Value::Null),
            ]),
            // The value must be a byte string.
            exact(vec![cbor_value(vec![("value", 1.into()), ("blob_id", ciborium::Value::Null)])]),
            // `blob_id` is nullable, but it must be present.
            exact(vec![cbor_value(vec![("value", ciborium::Value::Bytes(b"value".to_vec()))])]),
        ] {
            let error = decode_fetched(&body).unwrap_err();
            assert!(matches!(error, Error::MalformedResponse(_)), "{error:?}");
            assert_eq!(error.to_string(), "malformed response");
        }
    }

    fn client_for(listener: &TcpListener) -> Client {
        client_with_auth(listener, Arc::new(Anonymous))
    }

    fn fast_client_for(listener: &TcpListener) -> Client {
        client_with_retry(listener, Arc::new(Anonymous), FAST_RETRY)
    }

    fn client_with_auth(listener: &TcpListener, auth: Arc<dyn Auth>) -> Client {
        let port = listener.local_addr().unwrap().port();
        Client::new(&vt_str::format!("http://127.0.0.1:{port}/projects/test"), auth).unwrap()
    }

    fn client_with_retry(
        listener: &TcpListener,
        auth: Arc<dyn Auth>,
        retry: RetryConfig,
    ) -> Client {
        let port = listener.local_addr().unwrap().port();
        Client::with_retry(&vt_str::format!("http://127.0.0.1:{port}/projects/test"), auth, retry)
            .unwrap()
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
        let body = exact(vec![cbor_value(vec![
            ("value", ciborium::Value::Bytes(b"v".to_vec())),
            ("blob_id", ciborium::Value::Null),
        ])]);
        let server = std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", &body));

        assert_eq!(
            client.fetch(b"k", b"s").await.unwrap(),
            Some(Fetched::Exact { entries: vec![Entry { value: b"v".to_vec(), blob_id: None }] })
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
        let client = fast_client_for(&listener);
        let server = std::thread::spawn(move || {
            (0..3)
                .map(|_| serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"try later"))
                .collect::<Vec<_>>()
        });

        let error = client.fetch(b"k", b"s").await.unwrap_err();
        assert!(matches!(error, Error::Status(StatusCode::SERVICE_UNAVAILABLE, _)), "{error:?}");
        assert_eq!(std::error::Error::source(&error).unwrap().to_string(), "try later");
        assert_eq!(server.join().unwrap().len(), 3);
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
        let client = fast_client_for(&listener);
        let server = std::thread::spawn(move || {
            (0..4)
                .map(|_| {
                    serve_once(&listener, "HTTP/1.1 500 Internal Server Error", b"storage failed\n")
                })
                .collect::<Vec<_>>()
        });

        let err = client.store(b"k", b"s", b"v", Some(&blob)).await.unwrap_err();
        assert!(matches!(err, Error::Status(StatusCode::INTERNAL_SERVER_ERROR, _)));
        assert_eq!(err.to_string(), "HTTP status 500");
        assert_eq!(std::error::Error::source(&err).unwrap().to_string(), "storage failed");

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 4);
        let blob_part =
            b"name=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\narchive bytes\r\n";
        for request in &requests {
            assert!(request.starts_with(b"POST /projects/test/store HTTP/1.1\r\n"));
            assert!(contains(request, b"content-type: multipart/form-data; boundary="));
            assert!(contains(request, &metadata_part_bytes()));
            assert!(contains(request, blob_part));
        }
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

    /// Adds an `x-operation` header that names the request's operation.
    #[derive(Debug)]
    struct NamesOperation;

    impl Auth for NamesOperation {
        fn headers<'a>(&'a self, operation: Operation, _: &'a reqwest::Client) -> AuthHeaders<'a> {
            let name = match operation {
                Operation::Fetch => "fetch",
                Operation::Download => "download",
                Operation::Store => "store",
            };
            let headers = HeaderMap::from_iter([(
                HeaderName::from_static("x-operation"),
                HeaderValue::from_static(name),
            )]);
            Box::pin(std::future::ready(Ok(headers)))
        }
    }

    /// Run `send` with a client whose requests carry the headers from `auth`,
    /// and return the request it sent. The server responds with 404.
    async fn request_with_auth<F: Future<Output = ()>>(
        auth: Arc<dyn Auth>,
        send: impl FnOnce(Client) -> F,
    ) -> Vec<u8> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_with_auth(&listener, auth);
        let server =
            std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 404 Not Found", b""));
        send(client).await;
        server.join().unwrap()
    }

    #[tokio::test]
    async fn each_request_carries_the_auth_headers_for_its_operation() {
        let auth: Arc<dyn Auth> = Arc::new(NamesOperation);
        let fetch = request_with_auth(Arc::clone(&auth), |client| async move {
            assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
        })
        .await;
        assert!(contains(&fetch, b"x-operation: fetch\r\n"));

        let download = request_with_auth(Arc::clone(&auth), |client| async move {
            client.download("7").await.unwrap_err();
        })
        .await;
        assert!(contains(&download, b"x-operation: download\r\n"));

        let store = request_with_auth(auth, |client| async move {
            client.store(b"k", b"s", b"v", None).await.unwrap_err();
        })
        .await;
        assert!(contains(&store, b"x-operation: store\r\n"));
    }

    /// Fails to supply credentials for every request.
    #[derive(Debug)]
    struct Unavailable;

    impl Auth for Unavailable {
        fn headers<'a>(&'a self, _: Operation, _: &'a reqwest::Client) -> AuthHeaders<'a> {
            let error: AuthError = Arc::new(std::io::Error::other("no credentials"));
            Box::pin(std::future::ready(Err(error)))
        }
    }

    #[tokio::test]
    async fn requests_without_credentials_are_not_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_with_auth(&listener, Arc::new(Unavailable));

        let errors = [
            client.fetch(b"k", b"s").await.unwrap_err(),
            client.download("7").await.unwrap_err(),
            client.store(b"k", b"s", b"v", None).await.unwrap_err(),
        ];
        for error in errors {
            assert!(matches!(error, Error::Auth(_)), "{error:?}");
            assert_eq!(error.to_string(), "failed to authenticate");
            assert_eq!(std::error::Error::source(&error).unwrap().to_string(), "no credentials");
        }
        assert!(no_request_waiting(&listener));
    }

    /// Accept one connection and return the TLS record it starts with, which
    /// is the `ClientHello`, then close the connection without responding.
    fn read_client_hello(listener: &TcpListener) -> Vec<u8> {
        let (mut stream, _) = listener.accept().unwrap();
        let mut record = vec![0; 5];
        stream.read_exact(&mut record).unwrap();
        assert_eq!(record[0], 0x16, "not a TLS handshake record");
        let length = usize::from(u16::from_be_bytes([record[3], record[4]]));
        record.resize(5 + length, 0);
        stream.read_exact(&mut record[5..]).unwrap();
        record
    }

    #[tokio::test]
    async fn uses_http1_only_over_tls() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = Client::with_retry(
            &vt_str::format!("https://127.0.0.1:{port}/projects/test"),
            Arc::new(Anonymous),
            NO_RETRY,
        )
        .unwrap();
        let server = std::thread::spawn(move || read_client_hello(&listener));

        let error = client.fetch(b"k", b"s").await.unwrap_err();
        assert!(matches!(error, Error::Network(_)), "{error:?}");

        // HTTP/1.1 only: the ALPN extension lists http/1.1 and must not offer h2.
        let client_hello = server.join().unwrap();
        assert!(contains(&client_hello, b"\x08http/1.1"));
        assert!(!contains(&client_hello, b"\x02h2"));
    }

    #[tokio::test]
    async fn fetch_retries_transient_statuses_and_network_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = fast_client_for(&listener);
        let ok_body = exact(vec![cbor_value(vec![
            ("value", ciborium::Value::Bytes(b"v".to_vec())),
            ("blob_id", ciborium::Value::Null),
        ])]);
        let server = std::thread::spawn(move || {
            // Attempt 1: 503 with Retry-After: 60 (must not stall or fail).
            serve_raw_once(
                &listener,
                b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 60\r\nconnection: close\r\ncontent-length: 39\r\n\r\nService unavailable (concurrency_limit)",
            );
            // Attempt 2: connection dropped mid-response (network error).
            serve_raw_once(&listener, b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n");
            // Attempt 3: succeeds.
            serve_once(&listener, "HTTP/1.1 200 OK", &ok_body);
        });

        let start = Instant::now();
        assert_eq!(
            client.fetch(b"k", b"s").await.unwrap(),
            Some(Fetched::Exact { entries: vec![Entry { value: b"v".to_vec(), blob_id: None }] })
        );
        assert!(start.elapsed() < Duration::from_secs(5), "503 Retry-After: 60 should be ignored");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn download_retries_transient_error_before_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = fast_client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"busy");
            serve_once(&listener, "HTTP/1.1 200 OK", b"archive bytes");
        });

        let download = client.download("7").await.unwrap();
        assert_eq!(read_to_end(download).await.unwrap(), b"archive bytes");
        server.join().unwrap();
    }

    #[tokio::test]
    async fn store_retries_transient_errors_and_resends_blob() {
        let dir = tempfile::tempdir().unwrap();
        let blob = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        std::fs::write(blob.as_path(), b"archive bytes").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = fast_client_for(&listener);
        let server = std::thread::spawn(move || {
            let first = serve_raw_once(
                &listener,
                b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 60\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            // Drop connection before sending headers on attempt 2.
            {
                let (stream, _) = listener.accept().unwrap();
                drop(stream);
            }
            let third = serve_once(&listener, "HTTP/1.1 200 OK", b"");
            (first, third)
        });

        client.store(b"k", b"s", b"v", Some(&blob)).await.unwrap();
        let (first, third) = server.join().unwrap();
        let blob_part =
            b"name=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\narchive bytes\r\n";
        assert!(contains(&first, blob_part));
        assert!(contains(&third, blob_part));
    }

    #[tokio::test]
    async fn non_retryable_statuses_fail_without_retrying() {
        for status_line in [
            "HTTP/1.1 400 Bad Request",
            "HTTP/1.1 401 Unauthorized",
            "HTTP/1.1 403 Forbidden",
            "HTTP/1.1 413 Payload Too Large",
            "HTTP/1.1 501 Not Implemented",
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let client = fast_client_for(&listener);
            let server = {
                let listener = listener.try_clone().unwrap();
                std::thread::spawn(move || serve_once(&listener, status_line, b"nope"))
            };

            let error = client.store(b"k", b"s", b"v", None).await.unwrap_err();
            assert!(matches!(error, Error::Status(_, _)), "{status_line}: {error:?}");
            server.join().unwrap();
            assert!(no_request_waiting(&listener), "{status_line} should not retry");
        }
    }

    #[tokio::test]
    async fn download_once_does_not_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = fast_client_for(&listener);
        let server = {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"busy")
            })
        };

        let error = client.download_once("7").await.unwrap_err();
        assert!(matches!(error, Error::Status(StatusCode::SERVICE_UNAVAILABLE, _)), "{error:?}");
        server.join().unwrap();
        assert!(no_request_waiting(&listener));
    }

    #[tokio::test]
    async fn rate_limit_429_respects_retry_after() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cooldown = Duration::from_millis(80);
        let retry = RetryConfig {
            read: RetryPolicy { max_retry_after: cooldown, ..FAST_RETRY.read },
            store: RetryPolicy { max_retry_after: cooldown, ..FAST_RETRY.store },
            ..FAST_RETRY
        };
        let client = client_with_retry(&listener, Arc::new(Anonymous), retry);
        let server = std::thread::spawn(move || {
            // `Retry-After: 5` is clamped to `cooldown` (80 ms).
            serve_raw_once(
                &listener,
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 5\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            serve_once(&listener, "HTTP/1.1 200 OK", b"");
        });

        let start = Instant::now();
        client.store(b"k", b"s", b"v", None).await.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(60),
            "429 Retry-After cooldown was not respected: {:?}",
            start.elapsed()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rate_limit_429_pauses_same_kind_peers_without_stalling_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let cooldown = Duration::from_millis(80);
        let retry = RetryConfig {
            // Single attempt so the first store returns 429 immediately after
            // publishing the store cooldown; a read runs right away while a
            // peer store waits for the cooldown.
            read: RetryPolicy { max_retries: 0, max_retry_after: cooldown, ..FAST_RETRY.read },
            store: RetryPolicy { max_retries: 0, max_retry_after: cooldown, ..FAST_RETRY.store },
            ..FAST_RETRY
        };
        let client = client_with_retry(&listener, Arc::new(Anonymous), retry);
        let server = std::thread::spawn(move || {
            serve_raw_once(
                &listener,
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 5\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
            );
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"");
            serve_once(&listener, "HTTP/1.1 200 OK", b"");
        });

        let err = client.store(b"k", b"s", b"v", None).await.unwrap_err();
        assert!(matches!(err, Error::Status(StatusCode::TOO_MANY_REQUESTS, _)), "{err:?}");

        let fetch_start = Instant::now();
        assert_eq!(client.fetch(b"k", b"s").await.unwrap(), None);
        assert!(
            fetch_start.elapsed() < Duration::from_millis(50),
            "store 429 should not stall fetch: {:?}",
            fetch_start.elapsed()
        );

        let store_start = Instant::now();
        client.store(b"k", b"s", b"v", None).await.unwrap();
        assert!(
            store_start.elapsed() >= Duration::from_millis(40),
            "peer store did not wait for shared 429 cooldown: {:?}",
            store_start.elapsed()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn outage_breaker_short_circuits_after_repeated_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let retry = RetryConfig {
            outage_threshold: 2,
            outage_cooldown: Duration::from_secs(30),
            ..NO_RETRY
        };
        let client = client_with_retry(&listener, Arc::new(Anonymous), retry);
        let server = {
            let listener = listener.try_clone().unwrap();
            std::thread::spawn(move || {
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
                serve_once(&listener, "HTTP/1.1 503 Service Unavailable", b"down");
            })
        };

        for _ in 0..2 {
            let err = client.fetch(b"k", b"s").await.unwrap_err();
            assert!(matches!(err, Error::Status(StatusCode::SERVICE_UNAVAILABLE, _)), "{err:?}");
        }
        server.join().unwrap();

        // Third request short-circuits locally without hitting the server.
        let err = client.store(b"k", b"s", b"v", None).await.unwrap_err();
        assert!(matches!(err, Error::Unavailable), "{err:?}");
        assert_eq!(err.to_string(), "remote cache temporarily unavailable");
        assert!(no_request_waiting(&listener));
    }

    #[tokio::test]
    async fn concurrent_stores_are_capped_at_max_concurrent_stores() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = Arc::new(fast_client_for(&listener));
        let total_stores = MAX_CONCURRENT_STORES * 2;
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
        let (filled_tx, filled_rx) = std::sync::mpsc::channel::<()>();

        let server = {
            let in_flight = Arc::clone(&in_flight);
            let max_in_flight = Arc::clone(&max_in_flight);
            std::thread::spawn(move || {
                let mut handlers = Vec::with_capacity(total_stores);
                for _ in 0..total_stores {
                    let (mut stream, _) = listener.accept().unwrap();
                    let in_flight = Arc::clone(&in_flight);
                    let max_in_flight = Arc::clone(&max_in_flight);
                    let release_rx = Arc::clone(&release_rx);
                    let filled_tx = filled_tx.clone();
                    handlers.push(std::thread::spawn(move || {
                        let mut request = Vec::new();
                        let mut buf = [0; 4096];
                        let header_end = loop {
                            let n = stream.read(&mut buf).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buf[..n]);
                            if let Some(pos) =
                                request.windows(4).position(|window| window == b"\r\n\r\n")
                            {
                                break pos + 4;
                            }
                        };
                        let content_length: usize = std::str::from_utf8(&request[..header_end])
                            .unwrap()
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        while request.len() < header_end + content_length {
                            let n = stream.read(&mut buf).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buf[..n]);
                        }
                        let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_in_flight.fetch_max(current, Ordering::SeqCst);
                        if current == MAX_CONCURRENT_STORES {
                            let _ = filled_tx.send(());
                        }
                        let _ = release_rx.lock().unwrap().recv();
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 0\r\n\r\n")
                            .unwrap();
                    }));
                }
                for handler in handlers {
                    handler.join().unwrap();
                }
            })
        };

        let mut tasks = Vec::with_capacity(total_stores);
        for _ in 0..total_stores {
            let client = Arc::clone(&client);
            tasks.push(tokio::spawn(async move { client.store(b"k", b"s", b"v", None).await }));
        }

        // Wait until all `MAX_CONCURRENT_STORES` slots are occupied, then release
        // every request one by one.
        tokio::task::spawn_blocking(move || {
            filled_rx.recv().unwrap();
            for _ in 0..total_stores {
                release_tx.send(()).unwrap();
            }
        })
        .await
        .unwrap();

        for task in tasks {
            task.await.unwrap().unwrap();
        }
        server.join().unwrap();
        assert_eq!(max_in_flight.load(Ordering::SeqCst), MAX_CONCURRENT_STORES);
    }

    #[tokio::test]
    async fn network_errors_leave_out_the_url() {
        let endpoint = "http://user:password@127.0.0.1:0/projects/test?token=secret";
        let client = Client::with_retry(endpoint, Arc::new(Anonymous), NO_RETRY).unwrap();

        let error = client.fetch(b"k", b"s").await.unwrap_err();
        assert!(matches!(error, Error::Network(_)), "{error:?}");
        let messages =
            std::iter::successors(Some(&error as &dyn std::error::Error), |err| err.source())
                .map(|err| vt_str::format!("{err}"));
        for text in messages.chain([vt_str::format!("{error:?}")]) {
            assert!(!text.contains("password") && !text.contains("secret"), "{text}");
        }
    }
}

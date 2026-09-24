//! Client for the remote cache server API. Keys, values, and blobs are opaque
//! bytes; the caller decides what they contain.

use std::time::Duration;

use reqwest::{
    Response, StatusCode,
    header::CONTENT_TYPE,
    multipart::{Form, Part},
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use url::{ParseError, Url};
use vt_path::AbsolutePath;
use vt_str::Str;

/// Time allowed to establish a connection, including the TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for each read of a response. Until the response headers
/// arrive, it also bounds sending the request.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// The downloaded blob couldn't be written to its file.
    #[error("failed to write the blob")]
    WriteBlob(#[source] std::io::Error),
    /// No complete response arrived, for example because the connection
    /// failed, timed out, or closed before the whole body arrived.
    #[error("network error")]
    Network(#[source] reqwest::Error),
    /// The server responded with a status other than 200. The source is the
    /// message in the response body, if any.
    #[error("HTTP status {}", .0.as_u16())]
    Status(StatusCode, #[source] Option<ServerMessage>),
    /// The response body isn't a fetch response.
    #[error("malformed response")]
    MalformedResponse(#[source] ciborium::de::Error<std::io::Error>),
}

/// The message in the body of an error response.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ServerMessage(Str);

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

/// The result of a fetch.
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
    /// Neither key matched an entry.
    NotFound,
}

/// A client for one remote cache endpoint.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    fetch_url: Url,
    store_url: Url,
    /// `{endpoint}/blob`, to which each download appends a blob ID.
    blob_url: Url,
}

impl Client {
    /// Create a client for `endpoint`, a base URL that may include a
    /// namespace path, such as `https://cache.example.com/projects/my-project`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEndpoint`] if `endpoint` isn't a usable URL, or
    /// [`Error::HttpClient`] if the HTTP client can't be created.
    pub fn new(endpoint: &str) -> Result<Self, Error> {
        let endpoint = parse_endpoint(endpoint)?;
        let fetch_url = route_url(&endpoint, "fetch")?;
        let store_url = route_url(&endpoint, "store")?;
        let blob_url = route_url(&endpoint, "blob")?;
        // reqwest configures TLS with the process's default crypto provider.
        // Installing fails if one is already installed; vite-plus installs
        // ring too.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(Error::HttpClient)?;
        Ok(Self { http, fetch_url, store_url, blob_url })
    }

    /// Fetch the entry stored under `key` with `POST {endpoint}/fetch`,
    /// falling back to the entry associated with `secondary_key`.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the server responds with a
    /// status other than 200, or the response isn't a fetch response.
    pub async fn fetch(&self, key: &[u8], secondary_key: &[u8]) -> Result<Fetched, Error> {
        let body = encode_cbor(&FetchRequest { key, secondary_key });
        let response = self
            .http
            .post(self.fetch_url.clone())
            .header(CONTENT_TYPE, "application/cbor")
            .body(body)
            .send()
            .await
            .map_err(Error::Network)?;
        let body = check_status(response).await?.bytes().await.map_err(Error::Network)?;
        decode_fetched(&body)
    }

    /// Download the blob `blob_id` with `GET {endpoint}/blob/{blob_id}`,
    /// writing it to the file at `path`. The file is created after a 200
    /// response. If the download fails after that, it may be left incomplete.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, the server responds with a
    /// status other than 200, the body ends early, or the file can't be
    /// written.
    pub async fn download(&self, blob_id: &str, path: &AbsolutePath) -> Result<(), Error> {
        let mut url = self.blob_url.clone();
        url.path_segments_mut().map_err(|()| Error::InvalidEndpoint(None))?.push(blob_id);
        let response = self.http.get(url).send().await.map_err(Error::Network)?;
        let mut response = check_status(response).await?;
        let mut file = tokio::fs::File::create(path).await.map_err(Error::WriteBlob)?;
        while let Some(chunk) = response.chunk().await.map_err(Error::Network)? {
            file.write_all(&chunk).await.map_err(Error::WriteBlob)?;
        }
        file.flush().await.map_err(Error::WriteBlob)?;
        Ok(())
    }

    /// Store `value` under `key` with `POST {endpoint}/store`, uploading the
    /// file at `blob` as its blob. Fetches that match no key fall back to this
    /// entry through `secondary_key`.
    ///
    /// # Errors
    ///
    /// Returns an error if the blob file can't be opened, the request fails,
    /// or the server responds with a status other than 200.
    pub async fn store(
        &self,
        key: &[u8],
        secondary_key: &[u8],
        value: &[u8],
        blob: Option<&AbsolutePath>,
    ) -> Result<(), Error> {
        let metadata = StoreMetadata { key, secondary_key, value };
        let mut form = Form::new().part("metadata", metadata_part(&metadata));
        if let Some(blob) = blob {
            form = form.part("blob", blob_part(blob).await?);
        }
        let response = self
            .http
            .post(self.store_url.clone())
            .multipart(form)
            .send()
            .await
            .map_err(Error::Network)?;
        // The response's blob ID isn't needed. Read the body anyway, so the
        // connection can be reused.
        check_status(response).await?.bytes().await.map_err(Error::Network)?;
        Ok(())
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
            assert!(matches!(Client::new(endpoint), Err(Error::InvalidEndpoint(_))), "{endpoint}");
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

        let not_found = cbor_map(vec![("kind", "not_found".into())]);
        assert_eq!(decode_fetched(&not_found).unwrap(), Fetched::NotFound);
    }

    #[test]
    fn rejects_malformed_fetch_responses() {
        for body in [
            b"\xffnot cbor".to_vec(),
            cbor_map(vec![("kind", "unknown".into())]),
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

    /// Accept one HTTP request, respond with `status_line` and `body`, and
    /// return the raw request.
    fn serve_once(listener: &TcpListener, status_line: &str, body: &[u8]) -> Vec<u8> {
        let headers = vt_str::format!("{status_line}\r\ncontent-length: {}\r\n\r\n", body.len());
        serve_raw_once(listener, &[headers.as_bytes(), body].concat())
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
        let port = listener.local_addr().unwrap().port();
        Client::new(&vt_str::format!("http://127.0.0.1:{port}/projects/test")).unwrap()
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
        let body = cbor_map(vec![("kind", "not_found".into())]);
        let server = std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", &body));

        assert_eq!(client.fetch(b"k", b"s").await.unwrap(), Fetched::NotFound);

        let request = server.join().unwrap();
        assert!(request.starts_with(b"POST /projects/test/fetch HTTP/1.1\r\n"));
        assert!(contains(&request, b"content-type: application/cbor\r\n"));
        let keys = encode_cbor(&FetchRequest { key: b"k", secondary_key: b"s" });
        assert!(request.ends_with(&[b"\r\n\r\n".as_slice(), &keys].concat()));
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

    #[tokio::test]
    async fn download_writes_the_blob_to_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server =
            std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 200 OK", b"archive bytes"));

        client.download("7", &path).await.unwrap();

        let request = server.join().unwrap();
        assert!(request.starts_with(b"GET /projects/test/blob/7 HTTP/1.1\r\n"));
        assert_eq!(std::fs::read(path.as_path()).unwrap(), b"archive bytes");
    }

    #[tokio::test]
    async fn download_of_a_missing_blob_creates_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        let server = std::thread::spawn(move || {
            serve_once(&listener, "HTTP/1.1 404 Not Found", b"Blob not found")
        });

        let error = client.download("7", &path).await.unwrap_err();
        assert_eq!(error.to_string(), "HTTP status 404");
        assert!(!path.as_path().exists());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn incomplete_download_is_a_network_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = client_for(&listener);
        // The connection closes before the announced length arrives.
        let server = std::thread::spawn(move || {
            serve_raw_once(&listener, b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\npartial")
        });

        let error = client.download("7", &path).await.unwrap_err();
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
    }
}

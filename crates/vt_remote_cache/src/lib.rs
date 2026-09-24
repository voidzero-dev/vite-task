//! Client for the remote cache server API. Keys, values, and blobs are opaque
//! bytes; the caller decides what they contain.

use std::{sync::Arc, time::Duration};

use reqwest::{
    StatusCode, Url,
    multipart::{Form, Part},
};
use rustls_platform_verifier::BuilderVerifierExt as _;
use serde::Serialize;
use vt_path::AbsolutePath;

/// Time allowed to establish a connection, including the TLS handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for each read of a response. Until the response headers
/// arrive, it also bounds sending the request.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A failed remote cache operation. The messages name only the kind of
/// failure, so they are the same on every platform. The underlying error, if
/// any, is the source.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint isn't an HTTP or HTTPS URL that can have a path.
    #[error("invalid endpoint")]
    InvalidEndpoint,
    /// The HTTP client couldn't be created, for example because no root
    /// certificates could be loaded.
    #[error("failed to create the HTTP client")]
    HttpClient(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The blob file couldn't be opened.
    #[error("failed to read the blob")]
    ReadBlob(#[source] std::io::Error),
    /// No complete response arrived, for example because the connection
    /// failed or timed out.
    #[error("network error")]
    Network(#[source] reqwest::Error),
    /// The server responded with a status other than 200.
    #[error("HTTP status {}", .0.as_u16())]
    Status(StatusCode),
}

/// An entry to store. The server treats each field as opaque bytes.
#[derive(Debug, Serialize)]
pub struct Entry<'a> {
    /// Identifies the entry.
    #[serde(with = "serde_bytes")]
    pub key: &'a [u8],
    /// Associated with `key`, so fetches that match no key can fall back to
    /// this entry.
    #[serde(with = "serde_bytes")]
    pub secondary_key: &'a [u8],
    /// The stored value.
    #[serde(with = "serde_bytes")]
    pub value: &'a [u8],
}

/// A client for one remote cache endpoint.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    store_url: Url,
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
        let store_url = route_url(&endpoint, "store")?;
        let tls = tls_config(endpoint.scheme() == "https")?;
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|err| Error::HttpClient(err.into()))?;
        Ok(Self { http, store_url })
    }

    /// Store `entry` with `POST {endpoint}/store`, uploading the file at
    /// `blob` as its blob.
    ///
    /// # Errors
    ///
    /// Returns an error if the blob file can't be opened, the request fails,
    /// or the server responds with a status other than 200.
    pub async fn store(&self, entry: &Entry<'_>, blob: Option<&AbsolutePath>) -> Result<(), Error> {
        let mut form = Form::new().part("metadata", metadata_part(entry));
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
        let status = response.status();
        if status != StatusCode::OK {
            return Err(Error::Status(status));
        }
        // The response's blob ID isn't needed. Read the body anyway, so the
        // connection can be reused.
        response.bytes().await.map_err(Error::Network)?;
        Ok(())
    }
}

fn parse_endpoint(endpoint: &str) -> Result<Url, Error> {
    let url = Url::parse(endpoint).map_err(|_| Error::InvalidEndpoint)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidEndpoint);
    }
    Ok(url)
}

/// Append `route` to the endpoint's path, keeping its namespace path.
fn route_url(endpoint: &Url, route: &str) -> Result<Url, Error> {
    let mut url = endpoint.clone();
    url.path_segments_mut().map_err(|()| Error::InvalidEndpoint)?.pop_if_empty().push(route);
    Ok(url)
}

/// TLS settings using the ring crypto provider. HTTPS endpoints verify
/// certificates with the operating system's verifier. Plain HTTP endpoints
/// don't load the system's root certificates, so they work on systems that
/// have none.
fn tls_config(https: bool) -> Result<rustls::ClientConfig, Error> {
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|err| Error::HttpClient(err.into()))?;
    let builder = if https {
        builder.with_platform_verifier().map_err(|err| Error::HttpClient(err.into()))?
    } else {
        builder.with_root_certificates(rustls::RootCertStore::empty())
    };
    Ok(builder.with_no_client_auth())
}

fn encode_metadata(entry: &Entry<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(entry, &mut bytes).expect("encoding byte strings into a Vec can't fail");
    bytes
}

fn metadata_part(entry: &Entry<'_>) -> Part {
    Part::bytes(encode_metadata(entry)).mime_str("application/cbor").expect("valid MIME type")
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
            assert!(matches!(Client::new(endpoint), Err(Error::InvalidEndpoint)), "{endpoint}");
        }
    }

    #[test]
    fn metadata_is_a_cbor_map_of_byte_strings() {
        let entry = Entry { key: b"k", secondary_key: b"", value: &[0x00, 0xff] };
        let mut expected = vec![0xa3];
        expected.extend(b"\x63key\x41k");
        expected.extend(b"\x6dsecondary_key\x40");
        expected.extend(b"\x65value\x42\x00\xff");
        assert_eq!(encode_metadata(&entry), expected);
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    /// Accept one HTTP request, respond with `status_line`, and return the
    /// raw request.
    fn serve_once(listener: &TcpListener, status_line: &str) -> Vec<u8> {
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
            .expect("request has a content length");
        while request.len() < header_end + content_length {
            let n = stream.read(&mut buf).unwrap();
            assert_ne!(n, 0, "connection closed before the request body ended");
            request.extend_from_slice(&buf[..n]);
        }
        let response = vt_str::format!("{status_line}\r\ncontent-length: 0\r\n\r\n");
        stream.write_all(response.as_bytes()).unwrap();
        request
    }

    #[tokio::test]
    async fn store_posts_metadata_and_blob_parts() {
        let dir = tempfile::tempdir().unwrap();
        let blob = AbsolutePathBuf::new(dir.path().join("archive.tar.zst")).unwrap();
        std::fs::write(blob.as_path(), b"archive bytes").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server =
            std::thread::spawn(move || serve_once(&listener, "HTTP/1.1 500 Internal Server Error"));

        let client =
            Client::new(&vt_str::format!("http://127.0.0.1:{port}/projects/test")).unwrap();
        let entry = Entry { key: b"k", secondary_key: b"s", value: b"v" };
        let result = client.store(&entry, Some(&blob)).await;
        assert!(matches!(result, Err(Error::Status(StatusCode::INTERNAL_SERVER_ERROR))));
        assert_eq!(result.unwrap_err().to_string(), "HTTP status 500");

        let request = server.join().unwrap();
        assert!(request.starts_with(b"POST /projects/test/store HTTP/1.1\r\n"));
        assert!(contains(&request, b"content-type: multipart/form-data; boundary="));
        let metadata = [
            b"name=\"metadata\"\r\nContent-Type: application/cbor\r\n\r\n".as_slice(),
            &encode_metadata(&entry),
            b"\r\n",
        ]
        .concat();
        assert!(contains(&request, &metadata));
        let blob =
            b"name=\"blob\"\r\nContent-Type: application/octet-stream\r\n\r\narchive bytes\r\n";
        assert!(contains(&request, blob));
    }
}

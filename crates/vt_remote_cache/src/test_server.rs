//! A loopback HTTP server for tests, which serves one request at a time.

use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
};

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// Accept one HTTP request, respond with `status_line` and `body`, and
/// return the raw request. The response closes the connection, so the client
/// sends its next request on a new one.
pub fn serve_once(listener: &TcpListener, status_line: &str, body: &[u8]) -> Vec<u8> {
    let headers = vt_str::format!(
        "{status_line}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    serve_raw_once(listener, &[headers.as_bytes(), body].concat())
}

/// Accept one HTTP request, write `response`, close the connection, and
/// return the raw request.
pub fn serve_raw_once(listener: &TcpListener, response: &[u8]) -> Vec<u8> {
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

/// Whether no connection is waiting on `listener`, so no request was sent to
/// it since the last one served.
pub fn no_request_waiting(listener: &TcpListener) -> bool {
    listener.set_nonblocking(true).unwrap();
    let waiting = listener.accept();
    listener.set_nonblocking(false).unwrap();
    waiting.is_err_and(|err| err.kind() == std::io::ErrorKind::WouldBlock)
}

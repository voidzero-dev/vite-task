use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

const BASE_PATH: &str = "/projects/test";
const REQUEST_TOKEN: &str = "request-token";
/// A JWT with a fake signature that expires in 2100.
const TOKEN: &str = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjQxMDI0NDQ4MDB9.c2lnbmF0dXJl";

/// oidc-remote-cache \[`--without-oidc`\] `<command>` \[`<args>`...\]
///
/// Runs `<command>` with `VP_REMOTE_CACHE_URL` set to a loopback remote cache
/// that also serves a fake GitHub Actions OIDC token endpoint, and with
/// `ACTIONS_ID_TOKEN_REQUEST_URL` and `ACTIONS_ID_TOKEN_REQUEST_TOKEN` set to
/// request tokens from it, unless `--without-oidc` is given. Then exits with
/// the command's exit code, after printing a line for each request to stderr.
///
/// - `GET /token` responds with a token if the request carries the request
///   token and asks for the endpoint as the audience, and with 401 or 400
///   otherwise.
/// - `POST {BASE_PATH}/fetch` responds with 404, or 400 if it carries
///   credentials.
/// - `POST {BASE_PATH}/store` responds with 200 if it carries the token, 401
///   if it doesn't, and 400 if it contains the request token.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (with_oidc, args) = match args {
        [flag, rest @ ..] if flag == "--without-oidc" => (false, rest),
        _ => (true, args),
    };
    let [program, args @ ..] = args else {
        return Err("Usage: vtt oidc-remote-cache [--without-oidc] <command> [args...]".into());
    };

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let origin = format!("http://{}", listener.local_addr()?);
    let endpoint = format!("{origin}{BASE_PATH}");
    let log = Arc::new(Mutex::new(Vec::new()));
    {
        let endpoint = endpoint.clone();
        let log = Arc::clone(&log);
        std::thread::spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                handle(stream, &endpoint, &log);
            }
        });
    }

    let mut command = std::process::Command::new(program);
    command.args(args).env("VP_REMOTE_CACHE_URL", &endpoint);
    if with_oidc {
        command
            .env("ACTIONS_ID_TOKEN_REQUEST_URL", format!("{origin}/token?api-version=2.0"))
            .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", REQUEST_TOKEN);
    }
    let status = command.status()?;
    for line in log.lock().unwrap().iter() {
        eprintln!("{line}");
    }
    std::process::exit(status.code().unwrap_or(1));
}

/// Respond to one request and close the connection. The request is logged
/// before the response is written, so it's in `log` once the client has the
/// response.
fn handle(mut stream: TcpStream, endpoint: &str, log: &Mutex<Vec<String>>) -> Option<()> {
    let request = read_request(&mut stream)?;
    let head = String::from_utf8_lossy(&request.bytes[..request.head_len]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let (method, target) = (request_line.next()?, request_line.next()?);
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization").then(|| value.trim().to_owned())
    });
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let leaks_request_token =
        request.bytes.windows(REQUEST_TOKEN.len()).any(|window| window == REQUEST_TOKEN.as_bytes());

    let (prefix, route, status) = match (method, path.strip_prefix(BASE_PATH)) {
        ("GET", _) if path == "/token" => {
            let status = if authorization.as_deref() != Some(&format!("Bearer {REQUEST_TOKEN}")) {
                "401 Unauthorized"
            } else if query_value(query, "audience").as_deref() != Some(endpoint) {
                "400 Bad Request"
            } else {
                "200 OK"
            };
            ("github-oidc", path, status)
        }
        ("POST", Some(route @ "/fetch")) => (
            "remote-cache",
            route,
            if authorization.is_some() { "400 Bad Request" } else { "404 Not Found" },
        ),
        ("POST", Some(route @ "/store")) => {
            let status = if leaks_request_token {
                "400 Bad Request"
            } else if authorization.as_deref() == Some(&format!("Bearer {TOKEN}")) {
                "200 OK"
            } else {
                "401 Unauthorized"
            };
            ("remote-cache", route, status)
        }
        _ => ("remote-cache", path, "404 Not Found"),
    };
    let body = if prefix == "github-oidc" && status == "200 OK" {
        format!(r#"{{"value":"{TOKEN}"}}"#)
    } else {
        String::new()
    };
    let code = status.split(' ').next()?;
    log.lock().unwrap().push(format!("[{prefix}] {method} {route} {code}"));
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).ok()
}

struct Request {
    bytes: Vec<u8>,
    head_len: usize,
}

/// Read a request with a `content-length` body. `None` if the connection
/// closes first.
fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    let head_len = loop {
        let n = stream.read(&mut buf).ok().filter(|&n| n > 0)?;
        bytes.extend_from_slice(&buf[..n]);
        if let Some(pos) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let content_length: usize = String::from_utf8_lossy(&bytes[..head_len])
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    while bytes.len() < head_len + content_length {
        let n = stream.read(&mut buf).ok().filter(|&n| n > 0)?;
        bytes.extend_from_slice(&buf[..n]);
    }
    Some(Request { bytes, head_len })
}

/// The decoded value of the first `name` parameter in a URL-encoded query.
fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))?
    })
}

fn percent_decode(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.as_bytes();
    while let [byte, tail @ ..] = rest {
        match byte {
            b'%' => {
                let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                rest = &tail[2..];
            }
            b'+' => {
                bytes.push(b' ');
                rest = tail;
            }
            _ => {
                bytes.push(*byte);
                rest = tail;
            }
        }
    }
    String::from_utf8(bytes).ok()
}

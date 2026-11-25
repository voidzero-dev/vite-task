use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    io::{Read as _, Write as _},
    net::{Shutdown, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

const BASE_PATH: &str = "/projects/test";

/// `mock-remote-cache [options...] <command> [<args>...]`
///
/// Runs `<command>` with `VP_REMOTE_CACHE_URL` pointing at an in-process
/// loopback HTTP/1.1 remote cache server, then prints the recorded requests to
/// stderr and exits with the command's exit code.
///
/// Options:
/// - `--state-dir <dir>`: Persist stored entries and blobs in `<dir>/state.cbor`
///   across invocations.
/// - `--fault <route>=<kind>`: Inject a one-shot fault on `<route>` (`/fetch`,
///   `/store`, or `/blob`). Multiple `--fault` flags for the same route are
///   consumed in FIFO order. Supported `<kind>` values: `503`, `429`, `drop`,
///   and `truncate` (for `/blob`).
/// - `--always-fail <route>=<kind>`: Always apply `<kind>` to `<route>`.
/// - `--hold-stores <n>`: Hold incoming `POST /store` responses until `<n>`
///   stores are simultaneously in flight (and `--release-after-fetches` has
///   been satisfied), then release them without sleeping.
/// - `--release-after-fetches <m>`: Minimum number of `POST /fetch` requests
///   required before releasing held `POST /store` responses.
/// - `--summary`: Print sorted per-outcome counts (and `max_concurrent_stores`
///   when `--hold-stores` is set) instead of individual request lines.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args;
    let mut state_dir: Option<PathBuf> = None;
    let mut faults: BTreeMap<Route, VecDeque<Fault>> = BTreeMap::new();
    let mut always_fail: BTreeMap<Route, Fault> = BTreeMap::new();
    let mut hold_stores: usize = 0;
    let mut release_after_fetches: usize = 0;
    let mut summary = false;

    loop {
        match args {
            [flag, dir, rest @ ..] if flag == "--state-dir" => {
                state_dir = Some(PathBuf::from(dir));
                args = rest;
            }
            [flag, spec, rest @ ..] if flag == "--fault" => {
                let (route, fault) = parse_fault_spec(spec)?;
                faults.entry(route).or_default().push_back(fault);
                args = rest;
            }
            [flag, spec, rest @ ..] if flag == "--always-fail" => {
                let (route, fault) = parse_fault_spec(spec)?;
                always_fail.insert(route, fault);
                args = rest;
            }
            [flag, count, rest @ ..] if flag == "--hold-stores" => {
                hold_stores = count.parse()?;
                args = rest;
            }
            [flag, count, rest @ ..] if flag == "--release-after-fetches" => {
                release_after_fetches = count.parse()?;
                args = rest;
            }
            [flag, rest @ ..] if flag == "--summary" => {
                summary = true;
                args = rest;
            }
            _ => break,
        }
    }

    let [program, command_args @ ..] = args else {
        return Err(
            "Usage: vtt mock-remote-cache [--state-dir <dir>] [--fault <route>=<kind>]... [--always-fail <route>=<kind>]... [--hold-stores <n>] [--release-after-fetches <m>] [--summary] <command> [args...]"
                .into(),
        );
    };

    let storage = match &state_dir {
        Some(dir) => load_state(dir)?,
        None => StoredState::default(),
    };

    let server = Arc::new(Server {
        state_dir,
        state: Mutex::new(ServerState { storage, faults, always_fail, log: Vec::new() }),
        barrier: (
            Mutex::new(BarrierState {
                hold_target: hold_stores,
                release_after_fetches,
                in_flight: 0,
                max_in_flight: 0,
                fetches: 0,
                released: false,
            }),
            Condvar::new(),
        ),
    });

    ctrlc::set_handler(|| {})?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = std::format!("http://{}{BASE_PATH}", listener.local_addr()?);
    {
        let server = Arc::clone(&server);
        std::thread::spawn(move || {
            for stream in listener.incoming().filter_map(Result::ok) {
                let server = Arc::clone(&server);
                std::thread::spawn(move || server.serve(stream));
            }
        });
    }

    let status = std::process::Command::new(program)
        .args(command_args)
        .env("VP_REMOTE_CACHE_URL", endpoint)
        .status()?;

    if summary {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for line in &server.state.lock().unwrap().log {
            *counts.entry(line.clone()).or_default() += 1;
        }
        for (line, count) in counts {
            eprintln!("[remote-cache] {line} x{count}");
        }
        if hold_stores > 0 {
            let max_in_flight = server.barrier.0.lock().unwrap().max_in_flight;
            eprintln!("[remote-cache] max_concurrent_stores={max_in_flight}");
        }
    } else {
        for line in &server.state.lock().unwrap().log {
            eprintln!("[remote-cache] {line}");
        }
    }

    std::process::exit(status.code().unwrap_or(1));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    Fetch,
    Store,
    Blob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    ServiceUnavailable,
    TooManyRequests,
    DropConnection,
    TruncateBlob,
}

fn parse_fault_spec(spec: &str) -> Result<(Route, Fault), Box<dyn std::error::Error>> {
    let (route, kind) = spec
        .split_once('=')
        .ok_or_else(|| std::format!("invalid fault spec '{spec}', expected <route>=<kind>"))?;
    let route = match route {
        "/fetch" => Route::Fetch,
        "/store" => Route::Store,
        "/blob" => Route::Blob,
        _ => return Err(std::format!("unknown route '{route}'").into()),
    };
    let fault = match kind {
        "503" => Fault::ServiceUnavailable,
        "429" => Fault::TooManyRequests,
        "drop" => Fault::DropConnection,
        "truncate" => Fault::TruncateBlob,
        _ => return Err(std::format!("unknown fault kind '{kind}'").into()),
    };
    if fault == Fault::TruncateBlob && route != Route::Blob {
        return Err("truncate fault is only valid for /blob".into());
    }
    Ok((route, fault))
}

struct Server {
    state_dir: Option<PathBuf>,
    state: Mutex<ServerState>,
    barrier: (Mutex<BarrierState>, Condvar),
}

struct ServerState {
    storage: StoredState,
    faults: BTreeMap<Route, VecDeque<Fault>>,
    always_fail: BTreeMap<Route, Fault>,
    log: Vec<String>,
}

struct BarrierState {
    hold_target: usize,
    release_after_fetches: usize,
    in_flight: usize,
    max_in_flight: usize,
    fetches: usize,
    released: bool,
}

impl BarrierState {
    const fn should_release(&self) -> bool {
        self.in_flight >= self.hold_target && self.fetches >= self.release_after_fetches
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct StoredState {
    entries: Vec<StoredEntry>,
    blobs: Vec<Vec<u8>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredEntry {
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    value: Vec<u8>,
    blob_id: Option<String>,
}

#[derive(serde::Deserialize)]
struct FetchRequest {
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
}

#[derive(serde::Deserialize)]
struct StoreMetadata {
    #[serde(with = "serde_bytes")]
    key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    value: Vec<u8>,
}

#[derive(serde::Serialize)]
struct ExactResponse<'a> {
    kind: &'static str,
    entries: Vec<ResponseEntry<'a>>,
}

#[derive(serde::Serialize)]
struct ResponseEntry<'a> {
    #[serde(with = "serde_bytes")]
    value: &'a [u8],
    blob_id: Option<&'a str>,
}

fn state_file_path(dir: &std::path::Path) -> PathBuf {
    dir.join("state.cbor")
}

fn load_state(dir: &std::path::Path) -> Result<StoredState, Box<dyn std::error::Error>> {
    let path = state_file_path(dir);
    if !path.exists() {
        return Ok(StoredState::default());
    }
    let bytes = std::fs::read(path)?;
    Ok(ciborium::from_reader(bytes.as_slice())?)
}

fn save_state(
    dir: &std::path::Path,
    state: &StoredState,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let mut bytes = Vec::new();
    ciborium::into_writer(state, &mut bytes)?;
    std::fs::write(state_file_path(dir), bytes)?;
    Ok(())
}

impl Server {
    fn next_fault(&self, route: Route) -> Option<Fault> {
        let mut state = self.state.lock().unwrap();
        if let Some(&fault) = state.always_fail.get(&route) {
            return Some(fault);
        }
        state.faults.get_mut(&route).and_then(VecDeque::pop_front)
    }

    fn record(&self, method: &str, route: &str, outcome: &str) {
        self.state.lock().unwrap().log.push(std::format!("{method} {route} {outcome}"));
    }

    fn serve(&self, mut stream: TcpStream) {
        let Some(request) = read_request(&mut stream) else {
            return;
        };
        let path = request.target.split_once('?').map_or(request.target.as_str(), |(p, _)| p);
        let route_str = path.strip_prefix(BASE_PATH).unwrap_or(path);

        match (request.method.as_str(), route_str) {
            ("POST", "/fetch") => self.handle_fetch(&mut stream, &request.body),
            ("POST", "/store") => {
                self.handle_store(&mut stream, request.content_type.as_deref(), &request.body);
            }
            ("GET", rest) if rest.starts_with("/blob/") => {
                let blob_id = &rest["/blob/".len()..];
                self.handle_blob(&mut stream, rest, blob_id);
            }
            _ => {
                self.record(&request.method, route_str, "404");
                write_response(&mut stream, "HTTP/1.1 404 Not Found", &[], b"");
            }
        }
    }

    fn handle_fetch(&self, stream: &mut TcpStream, body: &[u8]) {
        if let Some(fault) = self.next_fault(Route::Fetch) {
            self.apply_fault(stream, "POST", "/fetch", fault);
            return;
        }

        // Notify any held stores once enough fetches have arrived.
        {
            let mut barrier = self.barrier.0.lock().unwrap();
            barrier.fetches += 1;
            let should_notify =
                barrier.hold_target > 0 && !barrier.released && barrier.should_release();
            if should_notify {
                barrier.released = true;
            }
            drop(barrier);
            if should_notify {
                self.barrier.1.notify_all();
            }
        }

        let Ok(fetch) = ciborium::from_reader::<FetchRequest, _>(body) else {
            self.record("POST", "/fetch", "400");
            write_response(stream, "HTTP/1.1 400 Bad Request", &[], b"invalid cbor");
            return;
        };

        let response_body = {
            let mut state = self.state.lock().unwrap();
            let entries: Vec<ResponseEntry<'_>> = state
                .storage
                .entries
                .iter()
                .rev()
                .filter(|entry| entry.key == fetch.key)
                .map(|entry| ResponseEntry {
                    value: &entry.value,
                    blob_id: entry.blob_id.as_deref(),
                })
                .collect();
            if entries.is_empty() {
                state.log.push("POST /fetch 404".to_owned());
                drop(state);
                None
            } else {
                let mut encoded = Vec::new();
                ciborium::into_writer(&ExactResponse { kind: "exact", entries }, &mut encoded)
                    .unwrap();
                state.log.push("POST /fetch 200".to_owned());
                drop(state);
                Some(encoded)
            }
        };

        if let Some(encoded) = response_body {
            write_response(
                stream,
                "HTTP/1.1 200 OK",
                &[("content-type", "application/cbor")],
                &encoded,
            );
        } else {
            write_response(stream, "HTTP/1.1 404 Not Found", &[], b"");
        }
    }

    fn handle_store(&self, stream: &mut TcpStream, content_type: Option<&str>, body: &[u8]) {
        if let Some(fault) = self.next_fault(Route::Store) {
            self.apply_fault(stream, "POST", "/store", fault);
            return;
        }

        let Some(parts) = content_type.and_then(|ct| parse_multipart(ct, body)) else {
            self.record("POST", "/store", "400");
            write_response(stream, "HTTP/1.1 400 Bad Request", &[], b"invalid multipart");
            return;
        };
        let Ok(metadata) = ciborium::from_reader::<StoreMetadata, _>(parts.metadata) else {
            self.record("POST", "/store", "400");
            write_response(stream, "HTTP/1.1 400 Bad Request", &[], b"invalid metadata");
            return;
        };

        // Hold concurrent stores on the Condvar if configured, then decrement
        // `in_flight` strictly BEFORE sending the response so the client
        // cannot release its semaphore permit and overlap the next store.
        {
            let mut barrier = self.barrier.0.lock().unwrap();
            barrier.in_flight += 1;
            barrier.max_in_flight = barrier.max_in_flight.max(barrier.in_flight);
            if barrier.hold_target > 0 && !barrier.released {
                if barrier.should_release() {
                    barrier.released = true;
                    self.barrier.1.notify_all();
                } else {
                    while !barrier.released {
                        barrier = self.barrier.1.wait(barrier).unwrap();
                    }
                }
            }
            barrier.in_flight -= 1;
        }

        {
            let mut state = self.state.lock().unwrap();
            let blob_id = parts.blob.map(|blob| {
                state.storage.blobs.push(blob.to_vec());
                state.storage.blobs.len().to_string()
            });
            state.storage.entries.push(StoredEntry {
                key: metadata.key,
                value: metadata.value,
                blob_id,
            });
            if let Some(dir) = &self.state_dir {
                save_state(dir, &state.storage).unwrap();
            }
            state.log.push("POST /store 200".to_owned());
        }

        write_response(stream, "HTTP/1.1 200 OK", &[], b"");
    }

    fn handle_blob(&self, stream: &mut TcpStream, route: &str, blob_id: &str) {
        let fault = self.next_fault(Route::Blob);
        if let Some(fault) = fault
            && fault != Fault::TruncateBlob
        {
            self.apply_fault(stream, "GET", route, fault);
            return;
        }

        let blob = {
            let state = self.state.lock().unwrap();
            blob_id
                .parse::<usize>()
                .ok()
                .and_then(|id| id.checked_sub(1))
                .and_then(|idx| state.storage.blobs.get(idx).cloned())
        };

        let Some(blob) = blob else {
            self.record("GET", route, "404");
            write_response(stream, "HTTP/1.1 404 Not Found", &[], b"");
            return;
        };

        if fault == Some(Fault::TruncateBlob) {
            let cut = (blob.len() / 2).min(blob.len().saturating_sub(1));
            self.record("GET", route, "truncate");
            let head = std::format!(
                "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                blob.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&blob[..cut]);
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }

        self.record("GET", route, "200");
        write_response(stream, "HTTP/1.1 200 OK", &[], &blob);
    }

    fn apply_fault(&self, stream: &mut TcpStream, method: &str, route: &str, fault: Fault) {
        match fault {
            Fault::ServiceUnavailable => {
                self.record(method, route, "503");
                write_response(
                    stream,
                    "HTTP/1.1 503 Service Unavailable",
                    &[("retry-after", "60")],
                    b"busy",
                );
            }
            Fault::TooManyRequests => {
                self.record(method, route, "429");
                write_response(
                    stream,
                    "HTTP/1.1 429 Too Many Requests",
                    &[("retry-after", "5")],
                    b"rate limited",
                );
            }
            Fault::DropConnection => {
                self.record(method, route, "drop");
                let _ = stream.shutdown(Shutdown::Both);
            }
            Fault::TruncateBlob => unreachable!("truncate is handled in handle_blob"),
        }
    }
}

struct HttpRequest {
    method: String,
    target: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Option<HttpRequest> {
    let mut raw = Vec::new();
    let mut buf = [0; 4096];
    let header_end = loop {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(pos) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let head = std::str::from_utf8(&raw[..header_end - 4]).ok()?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();

    let mut content_length: Option<usize> = None;
    let mut content_type: Option<String> = None;
    let mut chunked = false;

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().ok();
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }

    let mut body = raw.split_off(header_end);
    if let Some(expected) = content_length {
        while body.len() < expected {
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            body.extend_from_slice(&buf[..n]);
        }
        body.truncate(expected);
    } else if chunked {
        body = read_chunked_body(stream, body)?;
    }

    Some(HttpRequest { method, target, content_type, body })
}

fn read_chunked_body(stream: &mut TcpStream, mut buffered: Vec<u8>) -> Option<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut cursor = 0;
    let mut buf = [0; 4096];
    loop {
        let line_end = loop {
            if let Some(rel) = buffered[cursor..].windows(2).position(|w| w == b"\r\n") {
                break cursor + rel;
            }
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            buffered.extend_from_slice(&buf[..n]);
        };
        let size_str = std::str::from_utf8(&buffered[cursor..line_end]).ok()?;
        let size_hex = size_str.split(';').next()?.trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        cursor = line_end + 2;
        if size == 0 {
            return Some(decoded);
        }
        while buffered.len() < cursor + size + 2 {
            let n = stream.read(&mut buf).ok()?;
            if n == 0 {
                return None;
            }
            buffered.extend_from_slice(&buf[..n]);
        }
        decoded.extend_from_slice(&buffered[cursor..cursor + size]);
        cursor += size + 2;
    }
}

struct StoreParts<'a> {
    metadata: &'a [u8],
    blob: Option<&'a [u8]>,
}

fn parse_multipart<'a>(content_type: &str, body: &'a [u8]) -> Option<StoreParts<'a>> {
    let boundary = content_type.split(';').find_map(|param| {
        let (name, value) = param.trim().split_once('=')?;
        name.eq_ignore_ascii_case("boundary").then(|| value.trim().trim_matches('"'))
    })?;
    let delimiter = std::format!("--{boundary}");
    let delimiter = delimiter.as_bytes();

    let mut metadata = None;
    let mut blob = None;
    let mut pos = find_subslice(body, delimiter)?;

    loop {
        pos += delimiter.len();
        if body[pos..].starts_with(b"--") {
            break;
        }
        if !body[pos..].starts_with(b"\r\n") {
            return None;
        }
        pos += 2;

        let next = find_subslice(&body[pos..], delimiter)?;
        let part = &body[pos..pos + next];
        let part = part.strip_suffix(b"\r\n").unwrap_or(part);
        let head_end = find_subslice(part, b"\r\n\r\n")?;
        let part_head = std::str::from_utf8(&part[..head_end]).ok()?;
        let part_body = &part[head_end + 4..];

        if part_head.contains("name=\"metadata\"") {
            metadata = Some(part_body);
        } else if part_head.contains("name=\"blob\"") {
            blob = Some(part_body);
        }

        pos += next;
    }

    Some(StoreParts { metadata: metadata?, blob })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn write_response(
    stream: &mut TcpStream,
    status_line: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    let mut head = std::format!("{status_line}\r\nconnection: close\r\n");
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    let _ = write!(head, "content-length: {}\r\n\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

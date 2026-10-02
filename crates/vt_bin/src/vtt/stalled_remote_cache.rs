use std::{
    io::{Read as _, Write as _},
    net::TcpStream,
};

/// stalled-remote-cache \[`--fetch-miss`\] `<command>` \[`<args>`...\]
///
/// Runs `<command>` with `VP_REMOTE_CACHE_URL` set to a loopback endpoint
/// that accepts requests but never responds, then exits with the command's
/// exit code. With `--fetch-miss`, fetches get a 404, which is a miss, so only
/// the other requests, such as uploads, stall. Emits a "request" milestone
/// when a request that stalls arrives. Ctrl-C is left to the command.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (fetch_miss, args) = match args {
        [flag, args @ ..] if flag == "--fetch-miss" => (true, args),
        args => (false, args),
    };
    let [program, args @ ..] = args else {
        return Err("Usage: vtt stalled-remote-cache [--fetch-miss] <command> [args...]".into());
    };
    ctrlc::set_handler(|| {})?;

    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let endpoint = std::format!("http://{}/projects/test", listener.local_addr()?);
    std::thread::spawn(move || {
        for stream in listener.incoming().filter_map(Result::ok) {
            std::thread::spawn(move || serve(stream, fetch_miss));
        }
    });

    let status = std::process::Command::new(program)
        .args(args)
        .env("VP_REMOTE_CACHE_URL", endpoint)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Serve one request. A fetch gets a 404 that closes the connection if
/// `fetch_miss` is set. Otherwise, hold the connection without responding
/// until the client closes it.
fn serve(mut stream: TcpStream, fetch_miss: bool) {
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    let head_len = loop {
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => request.extend_from_slice(&buf[..n]),
            _ => return,
        }
    };
    let head = String::from_utf8_lossy(&request[..head_len]).into_owned();
    let mut lines = head.lines();
    let is_fetch = lines.next().is_some_and(|request_line| {
        request_line.starts_with("POST ") && request_line.contains("/fetch ")
    });

    if fetch_miss && is_fetch {
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or(0);
        // Read the whole body, so closing the connection doesn't reset it.
        while request.len() < head_len + content_length {
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => request.extend_from_slice(&buf[..n]),
                _ => return,
            }
        }
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        return;
    }

    pty_terminal_test_client::mark_milestone("request");
    while stream.read(&mut buf).is_ok_and(|n| n > 0) {}
}

use std::{
    io::{Read as _, Write as _},
    net::{Shutdown, TcpListener, TcpStream},
    sync::Arc,
};

/// stalled-remote-cache \[`--stall` `<route>`\]... `<command>` \[`<args>`...\]
///
/// Runs `<command>` with `VP_REMOTE_CACHE_URL` set to a loopback proxy for the
/// endpoint in `VP_REMOTE_CACHE_URL`, which must be
/// `http://<host>:<port>/<path>`, then exits with the command's exit code.
/// Requests to a stalled route below the endpoint, such as `/store`, are never
/// forwarded or answered: each emits a "stalled" milestone when it arrives,
/// and its connection is held until the client closes it. Other requests are
/// forwarded, each on its own connection. Ctrl-C is left to the command.
///
/// On Windows a milestone is the console title, which `ConPTY` sends when it
/// next renders, so if the command sets one at about the same time, the
/// earlier title can be lost.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args;
    let mut stalled_routes = Vec::new();
    while let [flag, route, rest @ ..] = args
        && flag == "--stall"
    {
        stalled_routes.push(route.clone());
        args = rest;
    }
    let [program, args @ ..] = args else {
        return Err(
            "Usage: vtt stalled-remote-cache [--stall <route>]... <command> [args...]".into()
        );
    };
    let upstream = std::env::var("VP_REMOTE_CACHE_URL")
        .map_err(|_| "VP_REMOTE_CACHE_URL must be set to the endpoint to proxy")?;
    let (authority, path) = upstream
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .ok_or("VP_REMOTE_CACHE_URL must be http://<host>:<port>/<path>")?;
    let proxy = Arc::new(Proxy {
        upstream: authority.to_owned(),
        base_path: std::format!("/{}", path.trim_end_matches('/')),
        stalled_routes,
    });
    ctrlc::set_handler(|| {})?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let endpoint = std::format!("http://{}{}", listener.local_addr()?, proxy.base_path);
    std::thread::spawn(move || {
        for stream in listener.incoming().filter_map(Result::ok) {
            let proxy = Arc::clone(&proxy);
            std::thread::spawn(move || proxy.serve(stream));
        }
    });

    let status = std::process::Command::new(program)
        .args(args)
        .env("VP_REMOTE_CACHE_URL", endpoint)
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

struct Proxy {
    /// `<host>:<port>` of the endpoint.
    upstream: String,
    /// The endpoint's path, without a trailing slash.
    base_path: String,
    /// Routes below `base_path` whose requests stall.
    stalled_routes: Vec<String>,
}

impl Proxy {
    /// Stall or forward the request on `client`. A forwarded request and its
    /// response get `connection: close`, so the client sends its next request
    /// on a new connection, which is served separately.
    fn serve(&self, mut client: TcpStream) {
        let Some((head, body_start)) = read_head(&mut client) else {
            return;
        };
        let target = head.split(' ').nth(1).unwrap_or_default();
        let path = target.split_once('?').map_or(target, |(path, _)| path);
        if path
            .strip_prefix(self.base_path.as_str())
            .is_some_and(|route| self.stalled_routes.iter().any(|stalled| stalled == route))
        {
            pty_terminal_test_client::mark_milestone("stalled");
            let mut buf = [0; 4096];
            while client.read(&mut buf).is_ok_and(|n| n > 0) {}
            return;
        }

        let Ok(mut upstream) = TcpStream::connect(self.upstream.as_str()) else {
            let _ = client.write_all(
                b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
            return;
        };
        let request_head = close_after_response(&head, Some(&self.upstream));
        if upstream.write_all(request_head.as_bytes()).is_err()
            || upstream.write_all(&body_start).is_err()
        {
            return;
        }
        let (Ok(mut client_reader), Ok(mut upstream_writer)) =
            (client.try_clone(), upstream.try_clone())
        else {
            return;
        };
        // The rest of the request body.
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut client_reader, &mut upstream_writer);
            let _ = upstream_writer.shutdown(Shutdown::Write);
        });

        if let Some((head, body_start)) = read_head(&mut upstream)
            && client.write_all(close_after_response(&head, None).as_bytes()).is_ok()
            && client.write_all(&body_start).is_ok()
        {
            let _ = std::io::copy(&mut upstream, &mut client);
        }
        let _ = client.shutdown(Shutdown::Both);
    }
}

/// Read an HTTP message's head, up to the blank line that ends it, and return
/// it with the bytes read after it.
fn read_head(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let rest = bytes.split_off(end + 4);
            return Some((String::from_utf8(bytes).ok()?, rest));
        }
        match stream.read(&mut buf) {
            Ok(n) if n > 0 => bytes.extend_from_slice(&buf[..n]),
            _ => return None,
        }
    }
}

/// `head` with `connection: close` in place of its `Connection` header, and
/// with `host` in place of its `Host` header if given.
fn close_after_response(head: &str, host: Option<&str>) -> String {
    let mut lines = head.split("\r\n").filter(|line| !line.is_empty());
    let mut rewritten = std::format!("{}\r\n", lines.next().unwrap_or_default());
    for line in lines {
        let name = line.split_once(':').map_or(line, |(name, _)| name).trim();
        if name.eq_ignore_ascii_case("connection")
            || (host.is_some() && name.eq_ignore_ascii_case("host"))
        {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    if let Some(host) = host {
        rewritten.push_str("host: ");
        rewritten.push_str(host);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("connection: close\r\n\r\n");
    rewritten
}

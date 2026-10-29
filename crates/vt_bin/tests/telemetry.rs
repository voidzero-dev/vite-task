use std::{
    env,
    io::{BufRead as _, Read as _, Write as _},
    net::TcpListener,
    process::Command,
    sync::mpsc,
    time::Duration,
};

use serde_json::json;
use vt_path::AbsolutePathBuf;

#[test]
fn session_flushes_on_success_and_failure_and_honors_export_opt_outs() {
    for (case, task, opt_out, success) in [
        ("success", "check", None, true),
        ("failure", "fail", None, false),
        ("sdk_disabled", "check", Some(("OTEL_SDK_DISABLED", "true")), true),
        ("exporter_none", "check", Some(("OTEL_TRACES_EXPORTER", "NONE")), true),
    ] {
        let directory = workspace();

        let cache = TcpListener::bind("127.0.0.1:0").unwrap();
        let cache_url =
            vt_str::format!("http://{}/cache?token=cache-secret", cache.local_addr().unwrap());
        let (cache_tx, cache_rx) = mpsc::channel();
        std::thread::spawn(move || {
            exchange(&cache, "404 Not Found");
            exchange(&cache, "200 OK");
            cache_tx.send(()).unwrap();
        });
        let collector = TcpListener::bind("127.0.0.1:0").unwrap();
        let collector_url = vt_str::format!("http://{}/v1/traces", collector.local_addr().unwrap());
        let (export_tx, export_rx) = mpsc::channel();
        let observer = collector.try_clone().unwrap();
        if opt_out.is_none() {
            std::thread::spawn(move || export_tx.send(exchange(&collector, "200 OK")).unwrap());
        }

        let utility = AbsolutePathBuf::new(env!("CARGO_BIN_EXE_vtt").into()).unwrap();
        let path = env::join_paths(
            std::iter::once(utility.parent().unwrap().as_path().to_path_buf())
                .chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_vt"));
        command
            .current_dir(directory.path())
            .args(["run", task])
            .env("PATH", path)
            .env("VP_REMOTE_CACHE", "read-write")
            .env("VP_REMOTE_CACHE_URL", cache_url.as_str())
            .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", collector_url.as_str())
            .env("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "x-test-key=ingest-secret")
            .env("OTEL_RESOURCE_ATTRIBUTES", "service.namespace=build")
            .env("OTEL_BSP_SCHEDULE_DELAY", "60000")
            .env_remove("OTEL_SERVICE_NAME")
            .env_remove("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL")
            .env_remove("OTEL_EXPORTER_OTLP_PROTOCOL")
            .env_remove("OTEL_SDK_DISABLED")
            .env_remove("OTEL_TRACES_EXPORTER")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
        if let Some((name, value)) = opt_out {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            success,
            "{case}: {}",
            std::str::from_utf8(&output.stderr).unwrap()
        );
        cache_rx.recv_timeout(Duration::from_secs(5)).expect("cache requests completed");
        if opt_out.is_some() {
            observer.set_nonblocking(true).unwrap();
            assert!(
                observer
                    .accept()
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::WouldBlock),
                "{case}: export was disabled"
            );
            continue;
        }

        let request = export_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("session flushed before process exit");
        let header_end = request.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
        assert!(
            std::str::from_utf8(&request[..header_end])
                .unwrap()
                .lines()
                .any(|line| line == "x-test-key: ingest-secret")
        );
        let exported: serde_json::Value = serde_json::from_slice(&request[header_end..]).unwrap();
        let resource = &exported["resourceSpans"][0];
        let attributes = resource["resource"]["attributes"].as_array().unwrap();
        for (key, value) in [("service.name", "vite-task"), ("service.namespace", "build")] {
            assert!(
                attributes
                    .iter()
                    .any(|attribute| attribute
                        == &json!({"key": key, "value": {"stringValue": value}})),
                "{case}: {key}"
            );
        }
        let spans = resource["scopeSpans"][0]["spans"].as_array().unwrap();
        assert_eq!(spans.len(), 2, "{case}: fetch and store were flushed");
    }
}

fn workspace() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("package.json"), br#"{"name":"traced-cache"}"#).unwrap();
    std::fs::write(
        directory.path().join("vite-task.json"),
        json!({
            "tasks": {
                "check": {
                    "command": "vtt print checked",
                    "cache": {"input": [], "output": []}
                },
                "fail": {
                    "command": "vtt exit 1", "cache": false,
                    "dependsOn": ["check"]
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    directory
}

/// Read a complete request and return a small HTTP response. Both the cache
/// and the OTLP receiver use ordinary TCP, including on Windows.
fn exchange(listener: &TcpListener, status: &str) -> Vec<u8> {
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut stream = std::io::BufReader::new(stream);
    let mut request = Vec::new();
    loop {
        assert_ne!(stream.read_until(b'\n', &mut request).unwrap(), 0);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let length = std::str::from_utf8(&request)
        .unwrap()
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let header_length = request.len();
    request.resize(header_length + length, 0);
    stream.read_exact(&mut request[header_length..]).unwrap();
    stream
        .get_mut()
        .write_all(
            vt_str::format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: 0\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    request
}

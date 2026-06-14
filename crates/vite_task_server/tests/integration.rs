use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    sync::Arc,
    thread,
};

use native_str::NativeStr;

#[cfg(unix)]
type RawStream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type RawStream = std::fs::File;
use rustc_hash::FxHashMap;
use tokio::runtime::Builder;
use vite_task_client::Client;
use vite_task_ipc_shared::Request;
use vite_task_server::{Error, Recorder, Reports, ServerHandle, serve};

fn env_map(pairs: &[(&str, &str)]) -> FxHashMap<Arc<OsStr>, Arc<OsStr>> {
    pairs
        .iter()
        .map(|(k, v)| (Arc::<OsStr>::from(OsStr::new(k)), Arc::<OsStr>::from(OsStr::new(v))))
        .collect()
}

fn run_with_server<F>(
    envs: FxHashMap<Arc<OsStr>, Arc<OsStr>>,
    client_work: F,
) -> Result<Reports, Error>
where
    F: FnOnce(Vec<(&'static OsStr, OsString)>) + Send + 'static,
{
    let recorder = Recorder::new(Arc::new(envs));

    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let (envs, ServerHandle { driver, stop_accepting }) = serve(recorder).expect("bind server");
        let envs: Vec<_> = envs.collect();

        let client = async move {
            tokio::task::spawn_blocking(move || client_work(envs))
                .await
                .expect("client work panicked");
            stop_accepting.signal();
        };

        let (result, ()) = tokio::join!(driver, client);
        result.map(Recorder::into_reports)
    })
}

fn connect(envs: &[(&'static OsStr, OsString)]) -> Client {
    Client::from_envs(envs.iter().map(|(k, v)| (k, v)))
        .expect("connect")
        .expect("serve should yield an IPC env")
}

/// Force a round-trip so the server has definitely processed every prior
/// fire-and-forget frame on this connection: frames on a single stream are
/// read sequentially, so once the server answers a `get_env` everything
/// before it must already have been dispatched to the handler.
fn flush(client: &Client) {
    let _ = client.get_env(OsStr::new("__VP_TEST_FLUSH__"), false).unwrap();
}

#[cfg(unix)]
fn connect_raw(name: &OsStr) -> RawStream {
    std::os::unix::net::UnixStream::connect(name).expect("connect raw")
}

#[cfg(windows)]
fn connect_raw(name: &OsStr) -> RawStream {
    std::fs::OpenOptions::new().read(true).write(true).open(name).expect("connect raw")
}

fn send_frame(stream: &mut RawStream, request: &Request<'_>) {
    let bytes = wincode::serialize(request).expect("serialize");
    let len = u32::try_from(bytes.len()).expect("frame length fits u32");
    stream.write_all(&len.to_le_bytes()).expect("write len");
    stream.write_all(&bytes).expect("write body");
    stream.flush().expect("flush");
}

#[test]
fn single_client_fire_and_forget() {
    #[cfg(unix)]
    let in_path = "/tmp/in.txt";
    #[cfg(windows)]
    let in_path = r"C:\tmp\in.txt";

    let reports = run_with_server(env_map(&[]), |envs| {
        let client = connect(&envs);
        client.ignore_input(OsStr::new(in_path)).unwrap();
        client.disable_cache().unwrap();
        flush(&client);
    })
    .expect("driver returned error");

    let inputs: Vec<_> = reports.ignored_inputs.iter().map(|p| p.as_path().as_os_str()).collect();
    assert_eq!(inputs, vec![OsStr::new(in_path)]);
    assert!(reports.cache_disabled);
}

#[test]
fn get_env_found_and_not_found() {
    let reports = run_with_server(env_map(&[("NODE_ENV", "production")]), |envs| {
        let client = connect(&envs);
        let present = client.get_env(OsStr::new("NODE_ENV"), true).unwrap();
        assert_eq!(present.as_deref(), Some(OsStr::new("production")));
        let missing = client.get_env(OsStr::new("MISSING"), false).unwrap();
        assert!(missing.is_none());
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    let node = reports.tracked_get_env.get(OsStr::new("NODE_ENV")).expect("NODE_ENV recorded");
    assert_eq!(node.as_deref(), Some(OsStr::new("production")));

    assert!(
        !reports.tracked_get_env.contains_key(OsStr::new("MISSING")),
        "untracked getEnv calls are not recorded"
    );
}

#[test]
fn get_env_untracked_then_tracked_records_once() {
    let reports = run_with_server(env_map(&[("NODE_ENV", "production")]), |envs| {
        let client = connect(&envs);
        let a = client.get_env(OsStr::new("NODE_ENV"), false).unwrap();
        let b = client.get_env(OsStr::new("NODE_ENV"), true).unwrap();
        let c = client.get_env(OsStr::new("NODE_ENV"), false).unwrap();
        for v in [a, b, c] {
            assert_eq!(v.as_deref(), Some(OsStr::new("production")));
        }
    })
    .expect("driver returned error");

    let node = reports.tracked_get_env.get(OsStr::new("NODE_ENV")).expect("recorded");
    assert_eq!(node.as_deref(), Some(OsStr::new("production")));
}

#[test]
fn concurrent_clients() {
    #[cfg(unix)]
    let paths = ["/tmp/worker_0", "/tmp/worker_1", "/tmp/worker_2", "/tmp/worker_3"];
    #[cfg(windows)]
    let paths = [r"C:\tmp\worker_0", r"C:\tmp\worker_1", r"C:\tmp\worker_2", r"C:\tmp\worker_3"];

    let reports = run_with_server(env_map(&[("SHARED", "value")]), move |envs| {
        let threads: Vec<_> = paths
            .iter()
            .map(|path| {
                let envs = envs.clone();
                let path = *path;
                thread::spawn(move || {
                    let client = connect(&envs);
                    client.ignore_input(OsStr::new(path)).unwrap();
                    let value = client.get_env(OsStr::new("SHARED"), true).unwrap();
                    assert_eq!(value.as_deref(), Some(OsStr::new("value")));
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    assert_eq!(reports.ignored_inputs.len(), 4);
    let shared = reports.tracked_get_env.get(OsStr::new("SHARED")).expect("recorded");
    assert_eq!(shared.as_deref(), Some(OsStr::new("value")));
}

#[test]
fn relative_input_joined_with_cwd() {
    let cwd = vite_path::current_dir().expect("cwd");
    let expected = cwd.as_path().join("sub/file.txt");

    let reports = run_with_server(env_map(&[]), |envs| {
        let client = connect(&envs);
        client.ignore_input(OsStr::new("sub/file.txt")).unwrap();
        flush(&client);
    })
    .expect("driver returned error");

    let inputs: Vec<_> = reports.ignored_inputs.iter().map(|p| p.as_path().as_os_str()).collect();
    assert_eq!(inputs, vec![expected.as_os_str()]);
}

#[test]
fn server_returns_error_on_non_absolute_path() {
    let err = run_with_server(env_map(&[]), |envs| {
        let name = &envs[0].1;
        let mut stream = connect_raw(name);

        let ns: Box<NativeStr> = OsStr::new("relative/path").into();
        send_frame(&mut stream, &Request::IgnoreInput(&ns));

        let mut buf = [0u8; 1];
        let read_err = stream.read_exact(&mut buf).expect_err("server should close connection");
        assert_eq!(read_err.kind(), io::ErrorKind::UnexpectedEof);
    })
    .expect_err("driver should surface the protocol error");

    match err {
        Error::NonAbsolutePath { path } => {
            assert_eq!(path, OsStr::new("relative/path"));
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

#[test]
fn get_envs_returns_matching_entries() {
    let reports = run_with_server(
        env_map(&[("PROBE_A", "alpha"), ("PROBE_B", "beta"), ("UNRELATED", "noise")]),
        |envs| {
            let client = connect(&envs);
            let matches = client.get_envs("PROBE_*", true).unwrap();
            assert_eq!(matches.len(), 2);
            assert_eq!(
                matches.get(OsStr::new("PROBE_A")).map(AsRef::as_ref),
                Some(OsStr::new("alpha"))
            );
            assert_eq!(
                matches.get(OsStr::new("PROBE_B")).map(AsRef::as_ref),
                Some(OsStr::new("beta"))
            );
            assert!(!matches.contains_key(OsStr::new("UNRELATED")));
        },
    )
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    let glob = reports.tracked_get_envs.get("PROBE_*").expect("glob recorded");
    assert_eq!(glob.matches.len(), 2);
}

#[test]
fn get_envs_empty_match_set_is_returned() {
    let reports = run_with_server(env_map(&[("FOO", "x"), ("BAR", "y")]), |envs| {
        let client = connect(&envs);
        let matches = client.get_envs("PROBE_*", false).unwrap();
        assert!(matches.is_empty());
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    assert!(
        !reports.tracked_get_envs.contains_key("PROBE_*"),
        "untracked getEnvs calls are not recorded"
    );
}

#[test]
fn get_envs_untracked_then_tracked_records_once() {
    let reports = run_with_server(env_map(&[("PROBE_A", "alpha")]), |envs| {
        let client = connect(&envs);
        let first = client.get_envs("PROBE_*", false).unwrap();
        let second = client.get_envs("PROBE_*", true).unwrap();
        let third = client.get_envs("PROBE_*", false).unwrap();
        assert_eq!(first, second);
        assert_eq!(second, third);
    })
    .expect("driver returned error");

    let glob = reports.tracked_get_envs.get("PROBE_*").expect("glob recorded");
    assert_eq!(glob.matches.len(), 1);
}

#[test]
fn get_envs_invalid_pattern_surfaces_error() {
    let err = run_with_server(env_map(&[]), |envs| {
        let client = connect(&envs);
        let send_err = client.get_envs("{unclosed", true).expect_err("server should reject");
        assert_eq!(send_err.kind(), io::ErrorKind::UnexpectedEof);
    })
    .expect_err("driver should surface the protocol error");

    match err {
        Error::InvalidGlob(inner) => {
            assert_eq!(inner.pattern.as_ref(), "{unclosed");
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
}

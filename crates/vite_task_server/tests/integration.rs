use std::{
    ffi::{OsStr, OsString},
    io,
    sync::Arc,
    thread,
};

use rustc_hash::FxHashMap;
use tokio::runtime::Builder;
use vite_task_client::Client;
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
    let _ = client.get_env(OsStr::new("__VP_TEST_FLUSH__")).unwrap();
}

#[test]
fn single_client_fire_and_forget() {
    let reports = run_with_server(env_map(&[]), |envs| {
        let client = connect(&envs);
        client.disable_cache().unwrap();
        flush(&client);
    })
    .expect("driver returned error");

    assert!(reports.cache_disabled);
}

#[test]
fn get_env_found_and_not_found() {
    let reports = run_with_server(env_map(&[("NODE_ENV", "production")]), |envs| {
        let client = connect(&envs);
        let present = client.get_env(OsStr::new("NODE_ENV")).unwrap();
        assert_eq!(present.as_deref(), Some(OsStr::new("production")));
        let missing = client.get_env(OsStr::new("MISSING")).unwrap();
        assert!(missing.is_none());
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    let node = reports.env_records.get(OsStr::new("NODE_ENV")).expect("NODE_ENV recorded");
    assert_eq!(node.as_deref(), Some(OsStr::new("production")));

    let missing = reports.env_records.get(OsStr::new("MISSING")).expect("MISSING recorded");
    assert!(missing.is_none());
}

#[test]
fn concurrent_clients() {
    let reports = run_with_server(env_map(&[("SHARED", "value")]), move |envs| {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let envs = envs.clone();
                thread::spawn(move || {
                    let client = connect(&envs);
                    let value = client.get_env(OsStr::new("SHARED")).unwrap();
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
    let shared = reports.env_records.get(OsStr::new("SHARED")).expect("recorded");
    assert_eq!(shared.as_deref(), Some(OsStr::new("value")));
}

#[test]
fn get_envs_returns_matching_entries() {
    let reports = run_with_server(
        env_map(&[("PROBE_A", "alpha"), ("PROBE_B", "beta"), ("UNRELATED", "noise")]),
        |envs| {
            let client = connect(&envs);
            let matches = client.get_envs("PROBE_*").unwrap();
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
}

#[test]
fn get_envs_empty_match_set_is_returned() {
    let reports = run_with_server(env_map(&[("FOO", "x"), ("BAR", "y")]), |envs| {
        let client = connect(&envs);
        let matches = client.get_envs("PROBE_*").unwrap();
        assert!(matches.is_empty());
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
}

#[test]
fn get_envs_invalid_pattern_surfaces_error() {
    let err = run_with_server(env_map(&[]), |envs| {
        let client = connect(&envs);
        let send_err = client.get_envs("{unclosed").expect_err("server should reject");
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

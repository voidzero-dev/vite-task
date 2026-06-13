use std::{
    ffi::{OsStr, OsString},
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
    let _ = client.get_env(OsStr::new("__VP_TEST_FLUSH__"), false).unwrap();
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
        let present = client.get_env(OsStr::new("NODE_ENV"), true).unwrap();
        assert_eq!(present.as_deref(), Some(OsStr::new("production")));
        let missing = client.get_env(OsStr::new("MISSING"), false).unwrap();
        assert!(missing.is_none());
    })
    .expect("driver returned error");

    assert!(!reports.cache_disabled);
    let node = reports.env_records.get(OsStr::new("NODE_ENV")).expect("NODE_ENV recorded");
    assert!(node.tracked);
    assert_eq!(node.value.as_deref(), Some(OsStr::new("production")));

    let missing = reports.env_records.get(OsStr::new("MISSING")).expect("MISSING recorded");
    assert!(!missing.tracked);
    assert!(missing.value.is_none());
}

#[test]
fn get_env_tracked_upgrade_is_monotonic() {
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

    let node = reports.env_records.get(OsStr::new("NODE_ENV")).expect("recorded");
    assert!(node.tracked, "tracked must remain true once set");
}

#[test]
fn concurrent_clients() {
    let reports = run_with_server(env_map(&[("SHARED", "value")]), move |envs| {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let envs = envs.clone();
                thread::spawn(move || {
                    let client = connect(&envs);
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
    let shared = reports.env_records.get(OsStr::new("SHARED")).expect("recorded");
    assert!(shared.tracked);
    assert_eq!(shared.value.as_deref(), Some(OsStr::new("value")));
}

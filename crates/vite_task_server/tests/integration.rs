use std::{
    ffi::{OsStr, OsString},
    sync::mpsc,
    thread,
};

use tokio::runtime::Builder;
use vite_task_client::Client;
use vite_task_server::{Error, Handler, Recorder, ServerHandle, serve};

fn run_with_server<H, F>(handler: H, client_work: F) -> Result<H, Error>
where
    H: Handler + 'static,
    F: FnOnce(Vec<(&'static OsStr, OsString)>) + Send + 'static,
{
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async move {
        let (envs, ServerHandle { driver, stop_accepting }) = serve(handler).expect("bind server");
        let envs: Vec<_> = envs.collect();

        let client = async move {
            tokio::task::spawn_blocking(move || client_work(envs))
                .await
                .expect("client work panicked");
            stop_accepting.signal();
        };

        let (result, ()) = tokio::join!(driver, client);
        result
    })
}

fn connect(envs: &[(&'static OsStr, OsString)]) -> Client {
    Client::from_envs(envs.iter().map(|(k, v)| (k, v)))
        .expect("connect")
        .expect("serve should yield an IPC env")
}

/// Wraps [`Recorder`] and reports every handled `disableCache` on a channel.
///
/// The protocol is fire-and-forget only, so there is no round-trip request a
/// test could use to flush the connection. Without the notification, a client
/// that sends and returns immediately could let the test signal stop-accepting
/// before the server has even accepted the connection, dropping the frame.
struct NotifyingRecorder {
    inner: Recorder,
    handled: mpsc::Sender<()>,
}

impl Handler for NotifyingRecorder {
    fn disable_cache(&mut self) {
        self.inner.disable_cache();
        let _ = self.handled.send(());
    }
}

#[test]
fn single_client_fire_and_forget() {
    let (tx, rx) = mpsc::channel();
    let handler =
        run_with_server(NotifyingRecorder { inner: Recorder::new(), handled: tx }, move |envs| {
            let client = connect(&envs);
            client.disable_cache().unwrap();
            // Hold the stop-accepting signal until the server has processed
            // the frame.
            rx.recv().unwrap();
        })
        .expect("driver returned error");

    assert!(handler.inner.into_reports().cache_disabled);
}

#[test]
fn concurrent_clients() {
    let (tx, rx) = mpsc::channel();
    let handler =
        run_with_server(NotifyingRecorder { inner: Recorder::new(), handled: tx }, move |envs| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let envs = envs.clone();
                    thread::spawn(move || {
                        let client = connect(&envs);
                        client.disable_cache().unwrap();
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            for _ in 0..4 {
                rx.recv().unwrap();
            }
        })
        .expect("driver returned error");

    assert!(handler.inner.into_reports().cache_disabled);
}

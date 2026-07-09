pub mod handler;
mod listener;

use std::{
    convert::Infallible,
    io::{self},
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
};

use futures_util::{
    future::{Either, select},
    pin_mut,
};
pub use handler::SeccompNotifyHandler;
use listener::NotifyListener;
use passfd::tokio::FdPassingExt;
use tokio::{
    net::UnixListener,
    sync::{oneshot, watch},
    task::{JoinHandle, JoinSet},
};
use tracing::{Level, span};

use crate::{
    bindings::alloc::alloc_seccomp_notif_resp,
    payload::{Architecture, Filter, SeccompPayload},
};

pub struct Supervisor<H> {
    payload: SeccompPayload,
    cancel_tx: oneshot::Sender<Infallible>,
    stop_handlers_tx: watch::Sender<bool>,
    handling_loop_task: JoinHandle<SupervisorOutcome<H>>,
}

pub struct SupervisorOutcome<H> {
    pub handlers: Vec<H>,
    pub error: Option<io::Error>,
}

impl<H> Supervisor<H> {
    #[must_use]
    pub const fn payload(&self) -> &SeccompPayload {
        &self.payload
    }

    /// Stops the supervisor and returns all handler instances plus the first
    /// error that made their observations incomplete.
    ///
    /// # Panics
    /// Panics if the handling loop task has panicked.
    pub async fn stop(self) -> SupervisorOutcome<H> {
        let _ = self.stop_handlers_tx.send(true);
        drop(self.cancel_tx);
        self.handling_loop_task.await.expect("handling loop task panicked")
    }
}

fn record_first_error(first_error: &mut Option<io::Error>, error: io::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

async fn handle_notifications<H: SeccompNotifyHandler>(
    mut listener: NotifyListener,
    mut handler: H,
    expected_architecture: Architecture,
    mut stop_rx: watch::Receiver<bool>,
) -> (H, Option<io::Error>) {
    let mut first_error = None;
    let mut resp_buf = alloc_seccomp_notif_resp();
    let expected_audit_architecture = expected_architecture.audit_value();

    loop {
        if *stop_rx.borrow() {
            tokio::spawn(drain_notifications(listener));
            break;
        }
        let notify_result = {
            let stop_future = stop_rx.changed();
            let notify_future = listener.next();
            pin_mut!(stop_future, notify_future);
            match select(stop_future, notify_future).await {
                Either::Left(_) => None,
                Either::Right((notify_result, _)) => {
                    Some(notify_result.map(Option::<&libc::seccomp_notif>::copied))
                }
            }
        };
        let Some(notify_result) = notify_result else {
            tokio::spawn(drain_notifications(listener));
            break;
        };
        let notify = match notify_result {
            Ok(Some(notify)) => notify,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "seccomp notification listener failed; tracing is incomplete"
                );
                record_first_error(&mut first_error, error);
                break;
            }
        };
        if *stop_rx.borrow() {
            let request_id = notify.id;
            tokio::spawn(async move {
                let mut resp_buf = alloc_seccomp_notif_resp();
                let _ = listener.send_continue(request_id, &mut resp_buf);
                drain_notifications(listener).await;
            });
            break;
        }
        let _span = span!(Level::TRACE, "notify loop tick");
        if notify.data.arch != expected_audit_architecture {
            let error =
                io::Error::new(io::ErrorKind::Unsupported, "unsupported syscall architecture");
            if first_error.is_none() {
                tracing::warn!(
                    request_id = notify.id,
                    architecture = notify.data.arch,
                    expected_architecture = expected_audit_architecture,
                    "unsupported syscall architecture; tracing is incomplete"
                );
            }
            record_first_error(&mut first_error, error);
        } else if expected_architecture.is_x32_syscall(notify.data.nr) {
            let error = io::Error::new(io::ErrorKind::Unsupported, "unsupported x32 syscall ABI");
            if first_error.is_none() {
                tracing::warn!(
                    request_id = notify.id,
                    syscall = notify.data.nr,
                    "unsupported x32 syscall ABI; tracing is incomplete"
                );
            }
            record_first_error(&mut first_error, error);
        } else if let Err(error) = handler.handle_notify(&notify) {
            tracing::warn!(
                request_id = notify.id,
                ?error,
                "failed to decode seccomp notification; tracing is incomplete"
            );
            record_first_error(&mut first_error, error);
        }
        let req_id = notify.id;
        if let Err(error) = listener.send_continue(req_id, &mut resp_buf) {
            tracing::warn!(
                ?error,
                "failed to continue seccomp notification; tracing is incomplete"
            );
            record_first_error(&mut first_error, error);
            break;
        }
    }

    (handler, first_error)
}

async fn drain_notifications(mut listener: NotifyListener) {
    let mut resp_buf = alloc_seccomp_notif_resp();
    loop {
        let request_id = match listener.next().await {
            Ok(Some(notify)) => notify.id,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(?error, "detached seccomp notification drainer failed");
                break;
            }
        };
        if let Err(error) = listener.send_continue(request_id, &mut resp_buf) {
            tracing::warn!(?error, "failed to continue detached seccomp notification");
            break;
        }
    }
}

/// Creates a new supervisor that listens for seccomp user notifications.
///
/// # Panics
/// Panics if the seccomp filter cannot be compiled or the target architecture is unsupported.
///
/// # Errors
/// Returns an error if the temporary IPC socket cannot be created.
pub fn supervise<H: SeccompNotifyHandler + Default + Send + 'static>(
    post_syscall_pc: u64,
) -> io::Result<Supervisor<H>> {
    let notify_listener = tempfile::Builder::new()
        .prefix("fspy_seccomp_notify")
        .make(|path| UnixListener::bind(path))?;

    let bpf_filter = Filter::new(
        Architecture::current(),
        H::syscalls().iter().map(syscalls::Sysno::id),
        post_syscall_pc,
    );
    let expected_architecture = Architecture::current();

    let payload = SeccompPayload {
        ipc_path: notify_listener.path().as_os_str().as_bytes().to_vec(),
        filter: bpf_filter,
        post_syscall_pc,
    };

    // The oneshot channel is used to cancel the accept loop.
    // The sender doesn't need to actually send anything. Drop is enough.
    let (cancel_tx, mut cancel_rx) = oneshot::channel::<Infallible>();
    let (stop_handlers_tx, stop_handlers_rx) = watch::channel(false);

    let handling_loop = async move {
        let mut join_set = JoinSet::new();
        let mut first_error = None;

        loop {
            let accept_future = notify_listener.as_file().accept();
            pin_mut!(accept_future);
            let incoming = match select(&mut cancel_rx, accept_future).await {
                Either::Left((Err(_), _)) => break,
                Either::Right((incoming, _)) => incoming,
            };
            let (incoming_stream, _) = match incoming {
                Ok(incoming) => incoming,
                Err(error) => {
                    record_first_error(&mut first_error, error);
                    break;
                }
            };
            let notify_fd = match incoming_stream.recv_fd().await {
                Ok(notify_fd) => notify_fd,
                Err(error) => {
                    record_first_error(&mut first_error, error);
                    continue;
                }
            };
            // SAFETY: `recv_fd` returns a valid file descriptor received via
            // Unix domain socket fd passing
            let notify_fd = unsafe { OwnedFd::from_raw_fd(notify_fd) };
            let listener = match NotifyListener::try_from(notify_fd) {
                Ok(listener) => listener,
                Err(error) => {
                    record_first_error(&mut first_error, error);
                    continue;
                }
            };

            join_set.spawn(handle_notifications(
                listener,
                H::default(),
                expected_architecture,
                stop_handlers_rx.clone(),
            ));
        }
        let mut handlers = Vec::<H>::new();
        while let Some(outcome) = join_set.join_next().await {
            match outcome {
                Ok((handler, error)) => {
                    handlers.push(handler);
                    if let Some(error) = error {
                        record_first_error(&mut first_error, error);
                    }
                }
                Err(error) => record_first_error(&mut first_error, io::Error::other(error)),
            }
        }
        SupervisorOutcome { handlers, error: first_error }
    };
    Ok(Supervisor {
        payload,
        cancel_tx,
        stop_handlers_tx,
        handling_loop_task: tokio::spawn(handling_loop),
    })
}

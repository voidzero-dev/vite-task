pub mod handler;
mod listener;

use std::{
    io,
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
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter};
use tokio::{
    net::UnixListener,
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::{Level, span};

use crate::{
    bindings::alloc::alloc_seccomp_notif_resp,
    payload::{Filter, SeccompPayload},
};

pub struct Supervisor<H> {
    payload: SeccompPayload,
    cancel_tx: watch::Sender<()>,
    handling_loop_task: JoinHandle<io::Result<Vec<H>>>,
}

impl<H> Supervisor<H> {
    #[must_use]
    pub const fn payload(&self) -> &SeccompPayload {
        &self.payload
    }

    /// Stops the supervisor and returns what every handler recorded so far.
    ///
    /// Does not wait for target processes to exit. A target that is still
    /// alive keeps its notifications answered by a detached task, so its
    /// filtered syscalls keep working; whatever it does from now on is not
    /// recorded.
    ///
    /// # Panics
    /// Panics if the handling loop task has panicked.
    ///
    /// # Errors
    /// Returns an error if any of the spawned handler tasks failed with an I/O error.
    pub async fn stop(self) -> io::Result<Vec<H>> {
        drop(self.cancel_tx);
        self.handling_loop_task.await.expect("handling loop task panicked")
    }
}

/// Creates a new supervisor that listens for seccomp user notifications.
///
/// # Panics
/// Panics if the seccomp filter cannot be compiled or the target architecture is unsupported.
///
/// # Errors
/// Returns an error if the temporary IPC socket cannot be created.
pub fn supervise<H: SeccompNotifyHandler + Default + Send + 'static>() -> io::Result<Supervisor<H>>
{
    let notify_listener = tempfile::Builder::new()
        .prefix("fspy_seccomp_notify")
        .make(|path| UnixListener::bind(path))?;

    let seccomp_filter = SeccompFilter::new(
        H::syscalls().iter().map(|sysno| (sysno.id().into(), vec![])).collect(),
        SeccompAction::Allow,
        SeccompAction::UserNotif,
        std::env::consts::ARCH.try_into().unwrap(),
    )
    .unwrap();

    let bpf_filter =
        Filter(BpfProgram::try_from(seccomp_filter).unwrap().into_iter().map(Into::into).collect());

    let payload = SeccompPayload {
        ipc_path: notify_listener.path().as_os_str().as_bytes().to_vec(),
        filter: bpf_filter,
    };

    // Dropping the sender cancels the accept loop and tells every handler task
    // to hand over its handler.
    let (cancel_tx, cancel_rx) = watch::channel(());

    let handling_loop = async move {
        // Every spawned task sends exactly one message: its handler when its
        // targets exited or when the supervisor stopped, or its I/O error.
        let (handler_tx, mut handler_rx) = mpsc::unbounded_channel::<io::Result<H>>();
        let mut spawned = 0usize;

        let mut accept_cancel = cancel_rx.clone();
        loop {
            let accept_future = notify_listener.as_file().accept();
            pin_mut!(accept_future);
            let cancelled = accept_cancel.changed();
            pin_mut!(cancelled);
            let (incoming_stream, _) = match select(cancelled, accept_future).await {
                Either::Left(_) => break,
                Either::Right((incoming, _)) => incoming?,
            };
            let notify_fd = incoming_stream.recv_fd().await?;
            // SAFETY: `recv_fd` returns a valid file descriptor received via
            // Unix domain socket fd passing
            let notify_fd = unsafe { OwnedFd::from_raw_fd(notify_fd) };
            let mut listener = NotifyListener::try_from(notify_fd)?;

            let mut handler = H::default();
            let mut resp_buf = alloc_seccomp_notif_resp();
            let mut task_cancel = cancel_rx.clone();
            let handler_tx = handler_tx.clone();
            spawned += 1;

            tokio::spawn(async move {
                /// What one select round decided, owned so the notify borrow
                /// ends before the listener is used again.
                enum Step {
                    Respond(u64),
                    Cancelled,
                    Eof,
                    Fail(io::Error),
                }

                loop {
                    let step = {
                        let next = listener.next();
                        pin_mut!(next);
                        let cancelled = task_cancel.changed();
                        pin_mut!(cancelled);
                        match select(cancelled, next).await {
                            Either::Left(_) => Step::Cancelled,
                            Either::Right((Ok(Some(notify)), _)) => {
                                let _span = span!(Level::TRACE, "notify loop tick");
                                // Errors on the supervisor side could be caused by a target process aborting.
                                // It shouldn't break the syscall handling loop as there might be target processes.
                                let _handle_result = handler.handle_notify(notify);
                                Step::Respond(notify.id)
                            }
                            Either::Right((Ok(None), _)) => Step::Eof,
                            Either::Right((Err(error), _)) => Step::Fail(error),
                        }
                    };
                    match step {
                        Step::Respond(req_id) => {
                            if let Err(error) = listener.send_continue(req_id, &mut resp_buf) {
                                let _ = handler_tx.send(Err(error));
                                return;
                            }
                        }
                        Step::Cancelled => {
                            // The supervisor stopped. Hand over what was
                            // recorded, then keep answering notifications so
                            // live targets keep working. Closing the notify fd
                            // instead would fail their filtered syscalls.
                            let _ = handler_tx.send(Ok(std::mem::take(&mut handler)));
                            while let Ok(Some(notify)) = listener.next().await {
                                let req_id = notify.id;
                                if listener.send_continue(req_id, &mut resp_buf).is_err() {
                                    return;
                                }
                            }
                            return;
                        }
                        Step::Eof => {
                            let _ = handler_tx.send(Ok(handler));
                            return;
                        }
                        Step::Fail(error) => {
                            let _ = handler_tx.send(Err(error));
                            return;
                        }
                    }
                }
            });
        }

        drop(handler_tx);
        let mut handlers = Vec::<H>::with_capacity(spawned);
        for _ in 0..spawned {
            let handler = handler_rx.recv().await.expect("every spawned task sends one message");
            handlers.push(handler?);
        }
        Ok(handlers)
    };
    Ok(Supervisor { payload, cancel_tx, handling_loop_task: tokio::spawn(handling_loop) })
}

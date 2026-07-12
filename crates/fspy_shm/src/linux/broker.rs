use std::{
    ffi::OsStr,
    future::Future,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    sync::Arc,
};

use nix::{
    sys::socket::{
        AddressFamily, SockFlag, SockType, UnixAddr, connect, getsockopt, socket,
        sockopt::PeerCredentials,
    },
    unistd::{Uid, geteuid},
};
use passfd::{FdPassingExt as SyncFdPassingExt, tokio::FdPassingExt as AsyncFdPassingExt};
use tokio::{net::UnixListener, task::JoinSet};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::{debug, warn};
use uuid::Uuid;

/// Creates the broker listener for `memfd` and returns the mapping id, the
/// broker task, and the guard whose drop stops the task.
///
/// The listener is bound eagerly, so the id is usable as soon as the task is
/// spawned; connections arriving earlier wait in the listen backlog.
pub(super) fn new(
    memfd: OwnedFd,
) -> io::Result<(String, impl Future<Output = ()> + Send + 'static, DropGuard)> {
    let id = Uuid::new_v4().simple().to_string();
    // Tokio treats a NUL-prefixed Unix socket path as a Linux abstract address.
    // This keeps the broker out of the filesystem and makes the id the complete address.
    let mut address = Vec::with_capacity(id.len() + 1);
    address.push(0);
    address.extend_from_slice(id.as_bytes());
    let listener = UnixListener::bind(OsStr::from_bytes(&address))?;

    let stop = CancellationToken::new();
    let service = run_broker(listener, memfd, geteuid(), stop.clone());
    Ok((id, service, stop.drop_guard()))
}

async fn run_broker(
    listener: UnixListener,
    memfd: OwnedFd,
    owner_uid: Uid,
    stop: CancellationToken,
) {
    let memfd = Arc::new(memfd);
    let mut sends = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = stop.cancelled() => return,
            _result = sends.join_next(), if !sends.is_empty() => {}
            client = listener.accept() => match client {
                Ok((client, _address)) => {
                    // Abstract sockets have no filesystem permissions, so authenticate the
                    // connecting process with the kernel-provided SO_PEERCRED credentials:
                    // https://man7.org/linux/man-pages/man7/unix.7.html
                    // D-Bus prefers the same mechanism because it requires no peer cooperation:
                    // https://gitlab.freedesktop.org/dbus/dbus/-/blob/958bf9db2100553bcd2fe2a854e1ebb42e886054/dbus/dbus-sysdeps-unix.c#L2296-2303
                    let credentials = match getsockopt(&client, PeerCredentials) {
                        Ok(credentials) => credentials,
                        Err(error) => {
                            debug!("shared-memory broker failed to read peer credentials: {error}");
                            continue;
                        }
                    };
                    if credentials.uid() != owner_uid.as_raw() {
                        debug!("shared-memory broker rejected a client owned by another user");
                        continue;
                    }
                    let memfd = Arc::clone(&memfd);
                    sends.spawn(async move {
                        if let Err(error) =
                            AsyncFdPassingExt::send_fd(&client, memfd.as_raw_fd()).await
                        {
                            debug!("shared-memory broker failed to send a descriptor: {error}");
                        }
                    });
                }
                Err(error) => {
                    warn!("shared-memory broker failed to accept a connection: {error}");
                    return;
                }
            },
        }
    }
}

pub(super) fn request_memfd(id: &str) -> io::Result<OwnedFd> {
    // Prevent the broker connection from leaking into later execs.
    let socket = socket(AddressFamily::Unix, SockType::Stream, SockFlag::SOCK_CLOEXEC, None)?;
    let address = UnixAddr::new_abstract(id.as_bytes())?;
    connect(socket.as_raw_fd(), &address)?;
    // `SCM_RIGHTS` does not preserve descriptor flags; `passfd` sets
    // `FD_CLOEXEC` on the received descriptor before returning it.
    let descriptor = SyncFdPassingExt::recv_fd(&socket.as_raw_fd())?;
    // SAFETY: passfd returns a newly received descriptor owned by the caller.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

#[cfg(test)]
mod tests {
    use std::{io, process::Command};

    use memfd::MemfdOptions;
    use nix::fcntl::{FcntlArg, FdFlag, SealFlag, fcntl};
    use subprocess_test::command_for_fn;

    use super::*;

    #[test]
    fn broker_construction_is_independent_of_long_tmpdir() {
        let command = command_for_fn!((), |(): ()| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_io()
                .enable_time()
                .build()
                .unwrap();
            let _guard = runtime.enter();
            let _owner = crate::create(4096).unwrap();
        });
        let status = std::thread::spawn(move || {
            Command::from(command)
                .env("TMPDIR", format!("/tmp/{}", "x".repeat(4096)))
                .status()
                .unwrap()
        })
        .join()
        .unwrap();
        assert!(status.success());
    }

    #[test]
    fn create_without_runtime_fails() {
        let error = match crate::create(4096) {
            Ok(_owner) => panic!("create without a runtime should fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("tokio runtime"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn broker_stops_when_guard_drops() {
        let memfd: OwnedFd = MemfdOptions::new()
            .close_on_exec(true)
            .create("shared-memory-test")
            .unwrap()
            .into_file()
            .into();
        let (_id, service, guard) = new(memfd).unwrap();
        let broker = tokio::spawn(service);
        tokio::task::yield_now().await;
        assert!(!broker.is_finished());

        drop(guard);
        tokio::time::timeout(std::time::Duration::from_secs(10), broker)
            .await
            .expect("broker should stop when the guard drops")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn received_descriptor_is_close_on_exec() {
        let owner = crate::create(4096).unwrap();
        let id = owner.id().to_owned();
        let descriptor = request_memfd_blocking(id).await.unwrap();
        assert!(
            FdFlag::from_bits_retain(fcntl(&descriptor, FcntlArg::F_GETFD).unwrap())
                .contains(FdFlag::FD_CLOEXEC)
        );
        assert_eq!(
            SealFlag::from_bits_retain(fcntl(&descriptor, FcntlArg::F_GET_SEALS).unwrap()),
            SealFlag::F_SEAL_GROW | SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_SEAL
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn broker_serves_concurrent_opens() {
        let owner = crate::create(4096).unwrap();
        let id = owner.id().to_owned();
        let clients = (0..12)
            .map(|_| {
                let id = id.clone();
                tokio::task::spawn_blocking(move || crate::open(&id))
            })
            .collect::<Vec<_>>();

        for client in clients {
            assert_eq!(client.await.unwrap().unwrap().len(), 4096);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn brokers_are_isolated_by_abstract_name() {
        let first = crate::create(4096).unwrap();
        let second = crate::create(4096).unwrap();
        let first_id = first.id().to_owned();
        let second_id = second.id().to_owned();

        let first_opened = open_blocking(first_id).await.unwrap();
        let second_opened = open_blocking(second_id).await.unwrap();
        // SAFETY: Both mappings are live and the accesses are in bounds and synchronized.
        unsafe {
            first.as_ptr().write(17);
            second.as_ptr().write(29);
            assert_eq!(first_opened.as_ptr().read(), 17);
            assert_eq!(second_opened.as_ptr().read(), 29);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn unavailable_ids_are_rejected() {
        let owner = crate::create(4096).unwrap();
        let id = owner.id().to_owned();

        assert!(open_blocking("not-a-broker-id".to_owned()).await.is_err());
        assert!(open_blocking("x".repeat(108)).await.is_err());
        assert!(open_blocking(id).await.is_ok());
    }

    async fn request_memfd_blocking(id: String) -> io::Result<OwnedFd> {
        tokio::task::spawn_blocking(move || request_memfd(&id)).await.unwrap()
    }

    async fn open_blocking(id: String) -> io::Result<crate::Shm> {
        tokio::task::spawn_blocking(move || crate::open(&id)).await.unwrap()
    }
}

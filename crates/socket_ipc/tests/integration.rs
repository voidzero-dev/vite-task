use std::io::{Read as _, Write as _};
#[cfg(unix)]
use std::sync::mpsc;

use socket_ipc::{Client, Server};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    runtime::Builder,
};

#[test]
fn round_trip() {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut server = Server::bind().expect("bind server");
        let name = server.name().to_owned();

        let client = tokio::task::spawn_blocking(move || {
            let mut client = Client::connect(&name).expect("connect client");
            client.write_all(b"ping").expect("write request");
            client.flush().expect("flush request");

            let mut response = [0; 4];
            client.read_exact(&mut response).expect("read response");
            assert_eq!(&response, b"pong");
        });

        let mut connection = server.accept().await.expect("accept client");
        let mut request = [0; 4];
        connection.read_exact(&mut request).await.expect("read request");
        assert_eq!(&request, b"ping");
        connection.write_all(b"pong").await.expect("write response");
        connection.flush().await.expect("flush response");

        client.await.expect("client task panicked");
    });
}

#[cfg(unix)]
#[test]
fn unix_uses_named_fifos() {
    use std::os::unix::fs::FileTypeExt as _;

    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut server = Server::bind().expect("bind server");
        let name = server.name().to_owned();
        let root = name.clone();
        let (connected_tx, connected_rx) = mpsc::channel();
        let (close_tx, close_rx) = mpsc::channel();

        let client = tokio::task::spawn_blocking(move || {
            let _client = Client::connect(&name).expect("connect client");
            connected_tx.send(()).expect("signal connected");
            close_rx.recv().expect("wait to close");
        });

        let connection = server.accept().await.expect("accept client");
        tokio::task::spawn_blocking(move || connected_rx.recv().expect("wait for client"))
            .await
            .expect("wait task panicked");

        let entries = std::fs::read_dir(root)
            .expect("read IPC root")
            .map(|entry| entry.expect("read IPC entry"))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 3, "rendezvous + request/response FIFO pair");
        assert!(
            entries.iter().all(|entry| entry.file_type().expect("read IPC entry type").is_fifo()),
            "every Unix endpoint must be a named FIFO"
        );

        close_tx.send(()).expect("close client");
        client.await.expect("client task panicked");
        drop(connection);
    });
}

/// A stale announcement from a client that died before the server got to it
/// must not fail the accept; the next client still connects.
#[cfg(unix)]
#[test]
fn accept_skips_announcement_from_dead_client() {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let mut server = Server::bind().expect("bind server");
        let name = server.name().to_owned();

        // Leftovers of a client that died right after announcing itself: its
        // FIFO pair exists on disk, its id sits in the rendezvous FIFO, and
        // every descriptor it held is closed.
        let id = [0xab; 16];
        let hex = "ab".repeat(16);
        let mode = nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR;
        nix::unistd::mkfifo(fifo_path(&name, &(hex.clone() + ".request")).as_os_str(), mode)
            .expect("create request FIFO");
        nix::unistd::mkfifo(fifo_path(&name, &(hex + ".response")).as_os_str(), mode)
            .expect("create response FIFO");
        std::fs::OpenOptions::new()
            .write(true)
            .open(fifo_path(&name, "connect"))
            .expect("open rendezvous FIFO")
            .write_all(&id)
            .expect("announce dead client");

        let client = tokio::task::spawn_blocking(move || {
            let mut client = Client::connect(&name).expect("connect client");
            client.write_all(b"ping").expect("write request");

            let mut response = [0; 4];
            client.read_exact(&mut response).expect("read response");
            assert_eq!(&response, b"pong");
        });

        let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), server.accept());
        let mut connection = accepted.await.expect("accept must not hang").expect("accept client");
        let mut request = [0; 4];
        connection.read_exact(&mut request).await.expect("read request");
        assert_eq!(&request, b"ping");
        connection.write_all(b"pong").await.expect("write response");

        client.await.expect("client task panicked");
    });
}

#[cfg(unix)]
fn fifo_path(root: &std::ffi::OsStr, name: &str) -> std::ffi::OsString {
    let mut path = root.to_os_string();
    path.push("/");
    path.push(name);
    path
}

/// The client must error out instead of hanging when the server disappears,
/// whether that happens before the connection attempt or in the middle of one.
#[test]
fn connect_fails_when_server_is_gone() {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        // Server already gone: the name no longer resolves.
        let server = Server::bind().expect("bind server");
        let name = server.name().to_owned();
        drop(server);
        let stale = tokio::task::spawn_blocking(move || Client::connect(&name));
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), stale)
            .await
            .expect("connect must not hang")
            .expect("client task panicked");
        assert!(result.is_err());

        // Server dies mid-connect, after the client reached it but before any
        // accept: the client must notice instead of waiting for a ready byte
        // that never comes. Only Unix has this wait state; a Windows client
        // either opens the pipe or fails at once, which the case above covers.
        #[cfg(unix)]
        {
            let server = Server::bind().expect("bind server");
            let name = server.name().to_owned();
            let root = name.clone();
            let midway = tokio::task::spawn_blocking(move || Client::connect(&name));
            // The client creates its request/response FIFO pair next to the
            // rendezvous FIFO on its way in; three entries mean it reached
            // the server.
            while std::fs::read_dir(&root).map_or(0, Iterator::count) < 3 {
                std::thread::yield_now();
            }
            drop(server);
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), midway)
                .await
                .expect("connect must not hang")
                .expect("client task panicked");
            assert!(result.is_err());
        }
    });
}

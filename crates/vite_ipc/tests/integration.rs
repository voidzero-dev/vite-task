use std::io::{Read as _, Write as _};
#[cfg(unix)]
use std::sync::mpsc;

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    runtime::Builder,
};
use vite_ipc::{Client, Server};

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

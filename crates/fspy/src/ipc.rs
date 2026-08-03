use std::io;

use fspy_shared::ipc::{NativeStr, PathAccess};
use pipe_socket::{Server, ServerConnection};
use tokio::{
    io::{AsyncReadExt as _, BufReader},
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::arena::PathAccessArena;

const FRAME_HEADER_LEN: usize = size_of::<u32>();

pub struct IpcSupervisor {
    server_name: Box<NativeStr>,
    cancellation_token: CancellationToken,
    task: Option<JoinHandle<io::Result<Vec<PathAccessArena>>>>,
}

impl IpcSupervisor {
    pub fn bind() -> io::Result<Self> {
        let server = Server::bind()?;
        let server_name = server.name().into();
        let cancellation_token = CancellationToken::new();
        let task = tokio::spawn(run_server(server, cancellation_token.clone()));
        Ok(Self { server_name, cancellation_token, task: Some(task) })
    }

    pub fn server_name(&self) -> &NativeStr {
        &self.server_name
    }

    pub async fn stop(mut self) -> io::Result<Vec<PathAccessArena>> {
        self.cancellation_token.cancel();
        self.task.take().expect("IPC supervisor task is missing").await.map_err(io::Error::other)?
    }
}

impl Drop for IpcSupervisor {
    fn drop(&mut self) {
        self.cancellation_token.cancel();
    }
}

async fn run_server(
    mut server: Server,
    cancellation_token: CancellationToken,
) -> io::Result<Vec<PathAccessArena>> {
    let mut readers = JoinSet::new();
    let mut arenas = Vec::new();

    loop {
        tokio::select! {
            biased;
            () = cancellation_token.cancelled() => break,
            result = readers.join_next(), if !readers.is_empty() => {
                collect_reader(result.expect("reader set is not empty"), &mut arenas)?;
            }
            connection = server.accept() => {
                readers.spawn(read_connection(connection?));
            }
        }
    }

    // Dropping the server prevents any new clients from connecting. Existing
    // connections remain alive in their reader tasks and are drained to EOF.
    drop(server);
    while let Some(result) = readers.join_next().await {
        collect_reader(result, &mut arenas)?;
    }
    Ok(arenas)
}

fn collect_reader(
    result: Result<io::Result<PathAccessArena>, tokio::task::JoinError>,
    arenas: &mut Vec<PathAccessArena>,
) -> io::Result<()> {
    arenas.push(result.map_err(io::Error::other)??);
    Ok(())
}

async fn read_connection(connection: ServerConnection) -> io::Result<PathAccessArena> {
    let mut connection = BufReader::new(connection);
    let mut arena = PathAccessArena::default();
    let mut frame = Vec::new();
    let mut header = [0; FRAME_HEADER_LEN];

    loop {
        if let Err(error) = connection.read_exact(&mut header).await {
            if connection_closed(&error) {
                return Ok(arena);
            }
            return Err(error);
        }

        let frame_len = u32::from_le_bytes(header) as usize;
        frame.resize(frame_len, 0);
        if let Err(error) = connection.read_exact(&mut frame).await {
            if connection_closed(&error) {
                return Ok(arena);
            }
            return Err(error);
        }

        let access: PathAccess<'_> = wincode::deserialize_exact(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        arena.add(access);
    }
}

fn connection_closed(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use fspy_shared::ipc::{AccessMode, NativePath, PathAccessSender};

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_rejects_new_connections_and_waits_for_existing_ones() {
        let supervisor = IpcSupervisor::bind().unwrap();
        let server_name = supervisor.server_name().to_cow_os_str().into_owned();
        let client_server_name = server_name.clone();
        let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
        let (close_tx, close_rx) = mpsc::channel();

        let client = tokio::task::spawn_blocking(move || {
            let mut sender = PathAccessSender::connect(&client_server_name).unwrap();
            sender.send(PathAccess { mode: AccessMode::READ, path: test_path() }).unwrap();
            connected_tx.send(()).unwrap();
            close_rx.recv().unwrap();
        });
        connected_rx.await.unwrap();

        let mut stop = tokio::spawn(supervisor.stop());
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut stop).await.is_err());
        let rejected = tokio::task::spawn_blocking(move || PathAccessSender::connect(&server_name))
            .await
            .unwrap();
        assert!(rejected.is_err());

        close_tx.send(()).unwrap();
        client.await.unwrap();
        let arenas = stop.await.unwrap().unwrap();
        let accesses =
            arenas.iter().flat_map(|arena| arena.borrow_accesses().iter()).collect::<Vec<_>>();
        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].mode, AccessMode::READ);
        assert_eq!(accesses[0].path, test_path());
    }

    #[cfg(unix)]
    fn test_path() -> &'static NativePath {
        std::path::Path::new("/fspy-pipe-socket-test").into()
    }

    #[cfg(windows)]
    fn test_path() -> &'static NativePath {
        NativePath::from_wide(&[
            b'\\' as u16,
            b'?' as u16,
            b'?' as u16,
            b'\\' as u16,
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            b't' as u16,
            b'e' as u16,
            b's' as u16,
            b't' as u16,
        ])
    }
}

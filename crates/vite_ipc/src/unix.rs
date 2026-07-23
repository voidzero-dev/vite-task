use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    pin::Pin,
    task::{Context, Poll},
};

use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::stat::Mode,
    unistd::mkfifo,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::unix::pipe,
};
use uuid::Uuid;
use vite_path::{AbsolutePath, AbsolutePathBuf};

const CONNECTION_ID_LEN: usize = 16;
const READY_BYTE: u8 = 1;
const CONNECT_FIFO_NAME: &str = "connect";
const REQUEST_FIFO_SUFFIX: &str = ".request";
const RESPONSE_FIFO_SUFFIX: &str = ".response";

pub struct Server {
    _dir: tempfile::TempDir,
    root: AbsolutePathBuf,
    rendezvous: pipe::Receiver,
    _rendezvous_keepalive: pipe::Sender,
}

impl Server {
    pub fn bind() -> io::Result<Self> {
        let dir = tempfile::Builder::new().prefix("vite_ipc_").tempdir()?;
        let root = AbsolutePath::new(dir.path())
            .expect("temp directories are absolute")
            .to_absolute_path_buf();
        let rendezvous_path = connect_fifo(&root);
        let mode = Mode::S_IRUSR | Mode::S_IWUSR;
        mkfifo(rendezvous_path.as_path(), mode).map_err(io::Error::from)?;

        // Keep one sender open so an idle rendezvous FIFO does not report EOF
        // between client announcements.
        let rendezvous = pipe::OpenOptions::new().open_receiver(&rendezvous_path)?;
        let rendezvous_keepalive = pipe::OpenOptions::new().open_sender(&rendezvous_path)?;

        Ok(Self { _dir: dir, root, rendezvous, _rendezvous_keepalive: rendezvous_keepalive })
    }

    pub fn name(&self) -> &OsStr {
        self.root.as_path().as_os_str()
    }

    pub async fn accept(&mut self) -> io::Result<ServerConnection> {
        let mut id = [0; CONNECTION_ID_LEN];
        self.rendezvous.read_exact(&mut id).await?;
        let paths = connection_paths(&self.root, ConnectionId::from_bytes(id));

        let reader = open_fifo_receiver(&paths.request)?;
        let mut writer = open_fifo_sender(&paths.response)?;
        writer.write_all(&[READY_BYTE])?;
        writer.flush()?;

        Ok(ServerConnection {
            reader: tokio::fs::File::from_std(reader),
            writer: tokio::fs::File::from_std(writer),
        })
    }
}

pub struct ServerConnection {
    reader: tokio::fs::File,
    writer: tokio::fs::File,
}

impl AsyncRead for ServerConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for ServerConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.writer).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

pub struct Client {
    reader: File,
    writer: File,
}

impl Client {
    pub fn connect(name: &OsStr) -> io::Result<Self> {
        let root = AbsolutePath::new(name).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "IPC server name is not absolute")
        })?;
        let id = ConnectionId::random();
        let paths = connection_paths(root, id);
        let mode = Mode::S_IRUSR | Mode::S_IWUSR;
        mkfifo(paths.request.as_path(), mode).map_err(io::Error::from)?;
        mkfifo(paths.response.as_path(), mode).map_err(io::Error::from)?;

        // The temporary readers let both writers open without blocking. They
        // stay alive until the ready byte confirms that the server has opened
        // its ends of both FIFOs.
        let request_bootstrap = OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&paths.request)?;
        let writer = OpenOptions::new().write(true).open(&paths.request)?;
        let response_bootstrap = OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&paths.response)?;

        let mut rendezvous = OpenOptions::new().write(true).open(connect_fifo(root))?;
        write_connection_id(&mut rendezvous, id)?;
        drop(rendezvous);

        let mut reader = OpenOptions::new().read(true).open(&paths.response)?;
        drop(response_bootstrap);

        let mut ready = [0];
        reader.read_exact(&mut ready)?;
        if ready[0] != READY_BYTE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid IPC rendezvous response",
            ));
        }
        drop(request_bootstrap);

        Ok(Self { reader, writer })
    }
}

impl Read for Client {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Write for Client {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Clone, Copy)]
struct ConnectionId(Uuid);

impl ConnectionId {
    fn random() -> Self {
        Self(Uuid::new_v4())
    }

    const fn from_bytes(bytes: [u8; CONNECTION_ID_LEN]) -> Self {
        Self(Uuid::from_bytes(bytes))
    }

    const fn as_bytes(&self) -> &[u8; CONNECTION_ID_LEN] {
        self.0.as_bytes()
    }
}

struct ConnectionPaths {
    request: AbsolutePathBuf,
    response: AbsolutePathBuf,
}

fn connect_fifo(root: &AbsolutePath) -> AbsolutePathBuf {
    root.join(CONNECT_FIFO_NAME)
}

fn connection_paths(root: &AbsolutePath, id: ConnectionId) -> ConnectionPaths {
    let mut encoded = [0; 32];
    let encoded = id.0.simple().encode_lower(&mut encoded);

    let mut request_name = OsString::from(&*encoded);
    request_name.push(REQUEST_FIFO_SUFFIX);
    let mut response_name = OsString::from(&*encoded);
    response_name.push(RESPONSE_FIFO_SUFFIX);

    ConnectionPaths { request: root.join(request_name), response: root.join(response_name) }
}

fn write_connection_id(rendezvous: &mut File, id: ConnectionId) -> io::Result<()> {
    // This fixed-size write is smaller than PIPE_BUF, so concurrent client IDs
    // cannot interleave.
    loop {
        match rendezvous.write(id.as_bytes()) {
            Ok(written) if written == id.as_bytes().len() => return Ok(()),
            Ok(_written) => {
                return Err(io::Error::new(io::ErrorKind::WriteZero, "short IPC rendezvous write"));
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
}

fn open_fifo_receiver(path: &AbsolutePath) -> io::Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(OFlag::O_NONBLOCK.bits()).open(path)?;
    set_blocking(&file)?;
    Ok(file)
}

fn open_fifo_sender(path: &AbsolutePath) -> io::Result<File> {
    let file = OpenOptions::new().write(true).custom_flags(OFlag::O_NONBLOCK.bits()).open(path)?;
    set_blocking(&file)?;
    Ok(file)
}

fn set_blocking(file: &File) -> io::Result<()> {
    let flags = OFlag::from_bits_retain(fcntl(file, FcntlArg::F_GETFL).map_err(io::Error::from)?);
    fcntl(file, FcntlArg::F_SETFL(flags - OFlag::O_NONBLOCK)).map_err(io::Error::from)?;
    Ok(())
}

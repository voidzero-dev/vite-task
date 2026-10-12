//! FIFO transport.
//!
//! Coding-agent sandboxes block Unix domain sockets, so the Unix transport is
//! built from named FIFOs, which are plain files that the sandboxes allow.
//!
//! The server owns a private `0700` temp directory. It holds one well-known
//! FIFO, `connect`, that the server reads for its whole life. A connection
//! works like this:
//!
//! 1. The client creates a FIFO pair, `<id>.request` and `<id>.response`,
//!    inside the server's directory.
//! 2. The client writes its 16-byte id to `connect`. The write is smaller
//!    than `PIPE_BUF`, so concurrent clients cannot interleave.
//! 3. The server reads the id, opens the pair's far ends, and writes one
//!    ready byte into the response FIFO. If the client died in the meantime,
//!    these steps fail and the server moves on to the next announcement.
//! 4. The client sees the ready byte and the connection is up: requests flow
//!    through one FIFO, responses through the other.
//!
//! FIFO opens block until the other end exists, and the handshake never
//! waits on that. The client opens the request FIFO read-write: POSIX
//! leaves read-write FIFO opens undefined, but Linux (see fifo(7)) and
//! macOS treat them as plain opens that never block. The response FIFO must
//! not get the same treatment: the client has to see end of file when the
//! server dies, which only happens if the client holds no write end of it,
//! so its read end opens nonblocking instead and stays for the whole
//! connection. The client also never waits on the server blindly: the
//! rendezvous open is nonblocking, and the wait for the ready byte watches
//! the rendezvous write end, which reports an error as soon as the server's
//! read end is gone. A dead server means a prompt error, never a hang.
//!
//! Writing to a FIFO with no read end open raises SIGPIPE, which kills a
//! process that left the signal at its default action. The client never
//! writes into that state: every FIFO it writes to has read access held for
//! the whole write, through the announcement descriptor or the request
//! descriptor itself. The server's writes rely on the Rust runtime instead,
//! which ignores SIGPIPE at startup, so a vanished client turns them into
//! plain errors.

use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{fd::AsFd, unix::fs::OpenOptionsExt},
    pin::Pin,
    task::{Context, Poll},
};

use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::stat::Mode,
    unistd::mkfifo,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::unix::pipe,
};
use uuid::Uuid;
use vt_path::{AbsolutePath, AbsolutePathBuf};

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
        let dir = tempfile::Builder::new().prefix("socket_ipc_").tempdir()?;
        let root = AbsolutePath::new(dir.path())
            .expect("temp directories are absolute")
            .to_absolute_path_buf();
        let rendezvous_path = connect_fifo(&root);
        let mode = Mode::S_IRUSR | Mode::S_IWUSR;
        mkfifo(rendezvous_path.as_path(), mode).map_err(io::Error::from)?;

        // Keep one sender open so an idle rendezvous FIFO does not report EOF
        // between client announcements. A single read-write descriptor would
        // do the same, but tokio only offers that on Linux.
        let rendezvous = pipe::OpenOptions::new().open_receiver(&rendezvous_path)?;
        let rendezvous_keepalive = pipe::OpenOptions::new().open_sender(&rendezvous_path)?;

        Ok(Self { _dir: dir, root, rendezvous, _rendezvous_keepalive: rendezvous_keepalive })
    }

    pub fn name(&self) -> &OsStr {
        self.root.as_path().as_os_str()
    }

    pub async fn accept(&mut self) -> io::Result<ServerConnection> {
        loop {
            let mut id = [0; CONNECTION_ID_LEN];
            self.rendezvous.read_exact(&mut id).await?;
            let paths = connection_paths(&self.root, Uuid::from_bytes(id));

            // A client can die between announcing itself and the handshake
            // finishing. Both opens are nonblocking and fail in that case,
            // and the failure belongs to that client alone: skip its
            // announcement and wait for the next one.
            let Ok(reader) = open_end(OpenOptions::new().read(true), &paths.request) else {
                continue;
            };
            let Ok(mut writer) = open_end(OpenOptions::new().write(true), &paths.response) else {
                continue;
            };
            if writer.write_all(&[READY_BYTE]).is_err() {
                continue;
            }

            // The connection does file I/O on the blocking thread pool
            // instead of registering the FIFOs with the reactor: kqueue on
            // macOS does not deliver events for FIFOs reliably, and a
            // reactor-registered connection reader never wakes up there.
            return Ok(ServerConnection {
                reader: tokio::fs::File::from_std(reader),
                writer: tokio::fs::File::from_std(writer),
            });
        }
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

        // Open the rendezvous FIFO first, before creating anything. A blocking
        // open would wait forever when the server is gone; a nonblocking write
        // open fails with ENXIO instead, because no reader holds the other
        // end. The descriptor is never written to; it exists for this check
        // and for the death watch in the poll below.
        let rendezvous = OpenOptions::new()
            .write(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(connect_fifo(root))
            .map_err(|error| {
                if error.raw_os_error() == Some(nix::errno::Errno::ENXIO as i32) {
                    server_is_gone()
                } else {
                    error
                }
            })?;

        let id = Uuid::new_v4();
        let paths = connection_paths(root, id);
        let mode = Mode::S_IRUSR | Mode::S_IWUSR;
        mkfifo(paths.request.as_path(), mode).map_err(io::Error::from)?;
        mkfifo(paths.response.as_path(), mode).map_err(io::Error::from)?;

        // A read-write open never blocks, so the client does not wait for
        // the server to open its read end. The client only writes to this
        // descriptor, and the read access it holds means those writes can
        // never raise SIGPIPE: after a server death they fill the FIFO's
        // buffer, and the next response read reports end of file.
        let writer = OpenOptions::new().read(true).write(true).open(&paths.request)?;

        // The connection's read end, nonblocking so this open does not wait
        // for the server either. Read-write would be wrong here: the client
        // must see end of file when the server dies, and that only happens
        // when no write end is left open. The descriptor also serves the
        // whole connection; reopening the FIFO after the ready byte could
        // wait forever for a writer when the server wrote the byte and
        // exited right after, while this descriptor reads the buffered byte
        // with no writer around.
        let mut reader = OpenOptions::new()
            .read(true)
            .custom_flags(OFlag::O_NONBLOCK.bits())
            .open(&paths.response)?;

        // The announcement goes through its own read-write descriptor: with
        // read access held, the FIFO has a reader for the whole write, so a
        // server that died after the liveness check cannot turn the write
        // into a SIGPIPE kill. It drops right after the write; keeping a
        // read end open would blind the death checks below, which report the
        // server as alive while any reader exists. The fixed-size write is
        // smaller than PIPE_BUF, so concurrent client IDs cannot interleave.
        let mut announce = OpenOptions::new().read(true).write(true).open(connect_fifo(root))?;
        loop {
            match announce.write(id.as_bytes()) {
                Ok(written) if written == id.as_bytes().len() => break,
                Ok(_written) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "short IPC rendezvous write",
                    ));
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
        drop(announce);

        // Wait for the server's ready byte while watching for its death. A
        // blocking read on the response FIFO could not see the death and
        // would wait forever.
        loop {
            let mut fds = [
                PollFd::new(reader.as_fd(), PollFlags::POLLIN),
                PollFd::new(rendezvous.as_fd(), PollFlags::empty()),
            ];
            // The timeout drives the liveness probe below. Linux reports a
            // dead server at once as POLLERR on the rendezvous write end;
            // macOS poll does not always report FIFO events, so the probe is
            // what catches it there.
            match poll(&mut fds, PollTimeout::from(100u16)) {
                Ok(_) => {}
                // A signal interrupted the poll; retry it instead of failing
                // the connect.
                Err(nix::errno::Errno::EINTR) => continue,
                Err(err) => return Err(err.into()),
            }

            if fds[0].revents().is_some_and(|events| events.contains(PollFlags::POLLIN)) {
                break;
            }
            if fds[1]
                .revents()
                .is_some_and(|events| events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP))
            {
                return Err(server_is_gone());
            }
            // Reopening the rendezvous for writing fails once the server's
            // read end is closed (ENXIO) or its directory is removed (ENOENT).
            if let Err(error) = OpenOptions::new()
                .write(true)
                .custom_flags(OFlag::O_NONBLOCK.bits())
                .open(connect_fifo(root))
            {
                return Err(match error.raw_os_error() {
                    Some(code)
                        if code == nix::errno::Errno::ENXIO as i32
                            || code == nix::errno::Errno::ENOENT as i32 =>
                    {
                        server_is_gone()
                    }
                    _ => error,
                });
            }
        }
        drop(rendezvous);

        // The reader was opened nonblocking to make the open safe; from here
        // on the connection wants plain blocking reads.
        set_blocking(&reader)?;

        let mut ready = [0];
        reader.read_exact(&mut ready)?;
        if ready[0] != READY_BYTE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid IPC rendezvous response",
            ));
        }

        Ok(Self { reader, writer })
    }
}

fn server_is_gone() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, "IPC server is gone")
}

/// Opens one end of a connection FIFO without blocking on the other side,
/// then restores plain blocking I/O for the connection's lifetime.
fn open_end(options: &mut OpenOptions, path: &AbsolutePath) -> io::Result<File> {
    let file = options.custom_flags(OFlag::O_NONBLOCK.bits()).open(path)?;
    set_blocking(&file)?;
    Ok(file)
}

fn set_blocking(file: &File) -> io::Result<()> {
    let flags = OFlag::from_bits_retain(fcntl(file, FcntlArg::F_GETFL).map_err(io::Error::from)?);
    fcntl(file, FcntlArg::F_SETFL(flags - OFlag::O_NONBLOCK)).map_err(io::Error::from)?;
    Ok(())
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

struct ConnectionPaths {
    request: AbsolutePathBuf,
    response: AbsolutePathBuf,
}

fn connect_fifo(root: &AbsolutePath) -> AbsolutePathBuf {
    root.join(CONNECT_FIFO_NAME)
}

fn connection_paths(root: &AbsolutePath, id: Uuid) -> ConnectionPaths {
    let mut encoded = [0; 32];
    let encoded = id.simple().encode_lower(&mut encoded);

    let mut request_name = OsString::from(&*encoded);
    request_name.push(REQUEST_FIFO_SUFFIX);
    let mut response_name = OsString::from(&*encoded);
    response_name.push(RESPONSE_FIFO_SUFFIX);

    ConnectionPaths { request: root.join(request_name), response: root.join(response_name) }
}

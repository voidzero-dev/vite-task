use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::windows::ffi::OsStrExt,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use winapi::um::namedpipeapi::WaitNamedPipeW;

pub struct Server {
    name: OsString,
    pending: NamedPipeServer,
}

impl Server {
    pub fn bind() -> io::Result<Self> {
        #[expect(
            clippy::disallowed_macros,
            reason = "the generated pipe name exceeds Str inline capacity"
        )]
        let name = OsString::from(format!(r"\\.\pipe\vite_ipc_{}", uuid::Uuid::new_v4()));
        let pending = ServerOptions::new().first_pipe_instance(true).create(&name)?;
        Ok(Self { name, pending })
    }

    pub fn name(&self) -> &OsStr {
        &self.name
    }

    pub async fn accept(&mut self) -> io::Result<ServerConnection> {
        self.pending.connect().await?;
        let next = ServerOptions::new().create(&self.name)?;
        Ok(ServerConnection { inner: std::mem::replace(&mut self.pending, next) })
    }
}

pub struct ServerConnection {
    inner: NamedPipeServer,
}

impl AsyncRead for ServerConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for ServerConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub struct Client {
    inner: File,
}

impl Client {
    pub fn connect(name: &OsStr) -> io::Result<Self> {
        // ERROR_PIPE_BUSY — see WinError.h. `std::io::Error` does not expose a
        // typed constant for it.
        const ERROR_PIPE_BUSY: i32 = 231;
        // NMPWAIT_WAIT_FOREVER — winapi 0.3 does not define NMPWAIT_*.
        const NMPWAIT_WAIT_FOREVER: u32 = 0xFFFF_FFFF;

        let mut wide: Vec<u16> = name.encode_wide().collect();
        wide.push(0);

        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(name) {
                Ok(inner) => return Ok(Self { inner }),
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    // SAFETY: `wide` is NUL-terminated and remains valid for
                    // the duration of the call.
                    let ok = unsafe { WaitNamedPipeW(wide.as_ptr(), NMPWAIT_WAIT_FOREVER) };
                    if ok == 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
}

impl Read for Client {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for Client {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

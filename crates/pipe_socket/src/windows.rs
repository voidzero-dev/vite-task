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
    /// The instance the next client will connect to. Replaced on every accept,
    /// and created before the accepted instance is handed out, so concurrent
    /// connect attempts never find the pipe without an instance.
    pending: NamedPipeServer,
    /// A separate instance that no client connects to. Its presence lets a
    /// client distinguish a busy data pipe from a server that stopped
    /// accepting while older connections are still draining.
    _liveness: NamedPipeServer,
}

impl Server {
    pub fn bind() -> io::Result<Self> {
        #[expect(
            clippy::disallowed_macros,
            reason = "the generated pipe name exceeds Str inline capacity"
        )]
        let name = OsString::from(format!(r"\\.\pipe\pipe_socket_{}", uuid::Uuid::new_v4()));
        let pending = ServerOptions::new().first_pipe_instance(true).create(&name)?;
        let liveness =
            ServerOptions::new().first_pipe_instance(true).create(liveness_name(&name))?;
        Ok(Self { name, pending, _liveness: liveness })
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
    /// Opens the named pipe as a client.
    ///
    /// Opening fails with `ERROR_PIPE_BUSY` when another client claimed the
    /// server's pending instance moments earlier, in the window before the
    /// server creates the next instance. Short `WaitNamedPipeW` waits hand
    /// that contention to the kernel while periodically checking the separate
    /// liveness pipe. There is no total timeout: a live server can remain busy
    /// indefinitely, while a stopped server is detected even if accepted data
    /// connections still exist.
    pub fn connect(name: &OsStr) -> io::Result<Self> {
        // ERROR_PIPE_BUSY, from WinError.h. `std::io::Error` has no typed
        // constant for it.
        const ERROR_PIPE_BUSY: i32 = 231;
        // ERROR_SEM_TIMEOUT, from WinError.h.
        const ERROR_SEM_TIMEOUT: i32 = 121;
        const LIVENESS_CHECK_INTERVAL_MS: u32 = 100;

        let wide = wide_name(name);
        let liveness_wide = wide_name(&liveness_name(name));

        loop {
            match std::fs::OpenOptions::new().read(true).write(true).open(name) {
                Ok(inner) => return Ok(Self { inner }),
                Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if !server_is_alive(&liveness_wide)? {
                        return Err(server_is_gone());
                    }

                    // SAFETY: `wide` is NUL-terminated and remains valid for
                    // the duration of the call.
                    let available =
                        unsafe { WaitNamedPipeW(wide.as_ptr(), LIVENESS_CHECK_INTERVAL_MS) };
                    if available == 0 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(ERROR_SEM_TIMEOUT) {
                            return Err(error);
                        }
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }
}

fn liveness_name(name: &OsStr) -> OsString {
    let mut name = name.to_os_string();
    name.push("_liveness");
    name
}

fn wide_name(name: &OsStr) -> Vec<u16> {
    name.encode_wide().chain([0]).collect()
}

fn server_is_alive(liveness_wide: &[u16]) -> io::Result<bool> {
    // ERROR_FILE_NOT_FOUND, from WinError.h.
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    // The liveness pipe always has one unclaimed instance, so a zero-timeout
    // wait succeeds while the server owns it and reports file-not-found after
    // the server is dropped or its process exits.
    // SAFETY: `liveness_wide` is NUL-terminated and valid for this call.
    if unsafe { WaitNamedPipeW(liveness_wide.as_ptr(), 0) } != 0 {
        return Ok(true);
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) { Ok(false) } else { Err(error) }
}

fn server_is_gone() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, "IPC server is gone")
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

#![doc = include_str!("../README.md")]

use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(any(unix, windows)))]
compile_error!("vite_ipc supports only Unix and Windows");

/// A named server that asynchronously accepts byte-stream connections.
pub struct Server {
    inner: imp::Server,
}

impl Server {
    /// Creates a server with a new unique name.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform transport cannot be created.
    pub fn bind() -> io::Result<Self> {
        imp::Server::bind().map(|inner| Self { inner })
    }

    /// Returns the opaque name clients use to connect to this server.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        self.inner.name()
    }

    /// Waits for and accepts the next client connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be accepted.
    pub async fn accept(&mut self) -> io::Result<ServerConnection> {
        self.inner.accept().await.map(|inner| ServerConnection { inner })
    }
}

/// The server side of an accepted byte-stream connection.
pub struct ServerConnection {
    inner: imp::ServerConnection,
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

/// A synchronous client byte stream connected by a server name.
pub struct Client {
    inner: imp::Client,
}

impl Client {
    /// Connects to the server identified by `name`.
    ///
    /// # Errors
    ///
    /// Returns an error if the name is invalid or the server cannot be reached.
    pub fn connect(name: &OsStr) -> io::Result<Self> {
        imp::Client::connect(name).map(|inner| Self { inner })
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

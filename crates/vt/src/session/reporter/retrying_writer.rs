//! A [`Write`] wrapper that waits and retries while its sink is temporarily full.

use std::io::{self, Write};

/// A sink that can wait until it accepts more bytes.
pub trait WaitWritable {
    /// Returns once a write to the sink is worth retrying.
    fn wait_writable(&self) -> io::Result<()>;
}

/// Writer that retries a write or flush that fails with [`io::ErrorKind::WouldBlock`].
///
/// `vp run` shares its stdout and stderr with the tasks that inherit them.
/// Such a task can switch the shared open file description to non-blocking
/// mode (Node.js does this when the stream is a pipe), and a write to a full
/// pipe then fails with `EAGAIN` instead of waiting.
///
/// The retry has to sit directly above the stream. A writer further up, such
/// as `LabeledWriter`, or a caller of `write_all` can't tell how many bytes a
/// failed call wrote, so it can't resume without repeating or dropping some.
pub struct RetryingWriter<W> {
    inner: W,
}

impl<W> RetryingWriter<W> {
    pub const fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: Write + WaitWritable> Write for RetryingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            match self.inner.write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    self.inner.wait_writable()?;
                }
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        loop {
            match self.inner.flush() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    self.inner.wait_writable()?;
                }
                result => return result,
            }
        }
    }
}

/// Sleeps until `fd` accepts a write, or until it reports an error or hangup,
/// which the retried write then returns.
#[cfg(unix)]
fn wait_fd_writable(fd: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    use nix::{
        errno::Errno,
        poll::{PollFd, PollFlags, PollTimeout, poll},
    };

    let mut fds = [PollFd::new(fd, PollFlags::POLLOUT)];
    match poll(&mut fds, PollTimeout::NONE) {
        Ok(_) | Err(Errno::EINTR) => Ok(()),
        Err(errno) => Err(errno.into()),
    }
}

macro_rules! impl_wait_writable_for_std_stream {
    ($stream:ty) => {
        impl WaitWritable for $stream {
            #[cfg(unix)]
            fn wait_writable(&self) -> io::Result<()> {
                use std::os::fd::AsFd as _;

                wait_fd_writable(self.as_fd())
            }

            // Windows has no shared non-blocking mode for a task to switch on,
            // so a `WouldBlock` isn't expected here. Yield and retry if one
            // appears anyway.
            #[cfg(windows)]
            fn wait_writable(&self) -> io::Result<()> {
                std::thread::yield_now();
                Ok(())
            }
        }
    };
}

impl_wait_writable_for_std_stream!(io::Stdout);
impl_wait_writable_for_std_stream!(io::Stderr);

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// A sink like a non-blocking pipe: it takes `capacity` bytes, then
    /// returns `WouldBlock` until `wait_writable` empties it.
    struct FullSink {
        capacity: usize,
        free: Cell<usize>,
        waits: Cell<usize>,
        flush_blocks: usize,
        written: Vec<u8>,
    }

    impl FullSink {
        fn new(capacity: usize) -> Self {
            Self {
                capacity,
                free: Cell::new(capacity),
                waits: Cell::new(0),
                flush_blocks: 0,
                written: Vec::new(),
            }
        }
    }

    impl Write for FullSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let n = buf.len().min(self.free.get());
            if n == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.free.set(self.free.get() - n);
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.flush_blocks == 0 {
                return Ok(());
            }
            self.flush_blocks -= 1;
            Err(io::ErrorKind::WouldBlock.into())
        }
    }

    impl WaitWritable for FullSink {
        fn wait_writable(&self) -> io::Result<()> {
            self.waits.set(self.waits.get() + 1);
            self.free.set(self.capacity);
            Ok(())
        }
    }

    #[test]
    fn write_all_waits_while_the_sink_is_full() {
        let mut writer = RetryingWriter::new(FullSink::new(3));
        writer.write_all(b"0123456789").unwrap();

        assert_eq!(writer.inner.written, b"0123456789");
        assert_eq!(writer.inner.waits.get(), 3);
    }

    #[test]
    fn flush_waits_while_the_sink_is_full() {
        let mut sink = FullSink::new(3);
        sink.flush_blocks = 2;
        let mut writer = RetryingWriter::new(sink);
        writer.flush().unwrap();

        assert_eq!(writer.inner.waits.get(), 2);
    }

    #[test]
    fn other_errors_are_returned() {
        struct Closed;

        impl Write for Closed {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }

        impl WaitWritable for Closed {
            fn wait_writable(&self) -> io::Result<()> {
                panic!("a broken pipe must not be retried");
            }
        }

        let mut writer = RetryingWriter::new(Closed);
        assert_eq!(writer.write(b"x").unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    /// The real case: a pipe in non-blocking mode that is full when the write
    /// starts. The reader starts only after the writer has begun to wait, so
    /// the write can't succeed without the retry.
    #[cfg(unix)]
    #[test]
    fn write_all_waits_for_a_full_non_blocking_pipe() {
        use std::{
            io::Read as _,
            os::fd::AsFd as _,
            sync::mpsc::{Sender, channel},
        };

        use nix::fcntl::{FcntlArg, OFlag, fcntl};

        struct PipeSink {
            pipe: io::PipeWriter,
            waiting: Sender<()>,
        }

        impl Write for PipeSink {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.pipe.write(buf)
            }

            fn flush(&mut self) -> io::Result<()> {
                self.pipe.flush()
            }
        }

        impl WaitWritable for PipeSink {
            fn wait_writable(&self) -> io::Result<()> {
                // The reader may already be gone; it only needs the first signal.
                let _ = self.waiting.send(());
                wait_fd_writable(self.pipe.as_fd())
            }
        }

        let (mut reader, mut pipe) = io::pipe().unwrap();
        let flags = OFlag::from_bits_retain(fcntl(pipe.as_fd(), FcntlArg::F_GETFL).unwrap());
        fcntl(pipe.as_fd(), FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();

        let mut filled = 0;
        loop {
            match pipe.write(&[b'.'; 4096]) {
                Ok(n) => filled += n,
                Err(err) => {
                    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
                    break;
                }
            }
        }

        let (waiting, started_waiting) = channel();
        let reader_thread = std::thread::spawn(move || {
            started_waiting.recv().unwrap();
            let mut received = Vec::new();
            reader.read_to_end(&mut received).unwrap();
            received
        });

        let payload: Vec<u8> = (0..=u8::MAX).cycle().take(1024 * 1024).collect();
        let mut writer = RetryingWriter::new(PipeSink { pipe, waiting });
        writer.write_all(&payload).unwrap();
        writer.flush().unwrap();
        drop(writer);

        let received = reader_thread.join().unwrap();
        assert_eq!(received.len(), filled + payload.len());
        assert_eq!(&received[filled..], payload);
    }
}

use std::{
    cell::RefCell,
    ffi::OsStr,
    io::{self, Write},
};

use vite_task_ipc_shared::{IPC_ENV_NAME, Request};

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::fs::File;

pub struct Client {
    stream: RefCell<Stream>,
}

impl Client {
    /// Scans `envs` for the runner's IPC connection info and connects if
    /// present. Typical callers pass `std::env::vars_os()`.
    ///
    /// Returns `Ok(None)` if the IPC env is absent (running outside the runner).
    /// `Err(..)` if the env is set but connecting fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the env var is set but the server cannot be reached.
    pub fn from_envs(
        envs: impl Iterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
    ) -> io::Result<Option<Self>> {
        for (name, value) in envs {
            if name.as_ref() == IPC_ENV_NAME {
                let stream = connect(value.as_ref())?;
                return Ok(Some(Self::from_stream(stream)));
            }
        }
        Ok(None)
    }

    const fn from_stream(stream: Stream) -> Self {
        Self { stream: RefCell::new(stream) }
    }

    /// Fire-and-forget: the call returns once the request is flushed to the
    /// kernel pipe buffer; the runner processes it during its drain phase
    /// after this process exits. See the `Request` type in the IPC protocol
    /// crate for why this is safe.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails to send.
    pub fn disable_cache(&self) -> io::Result<()> {
        self.send(&Request::DisableCache)
    }

    fn send(&self, request: &Request) -> io::Result<()> {
        let bytes = wincode::serialize(request)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let len = u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request too large"))?;
        let mut stream = self.stream.borrow_mut();
        stream.write_all(&len.to_le_bytes())?;
        stream.write_all(&bytes)?;
        stream.flush()?;
        Ok(())
    }
}

#[cfg(unix)]
fn connect(name: &OsStr) -> io::Result<Stream> {
    std::os::unix::net::UnixStream::connect(name)
}

/// Open a Windows named pipe as a client.
///
/// `OpenOptions::open` on a named-pipe path fails with `ERROR_PIPE_BUSY` when
/// the server's only pending instance has just been claimed by another client
/// — the brief window between the server accepting one connection and creating
/// the next instance. On `ERROR_PIPE_BUSY` we hand off to the kernel via
/// `WaitNamedPipeW`, which blocks until an instance becomes available (or
/// fails if the named pipe is gone). No polling and no arbitrary timeouts.
///
/// This matches what the `interprocess` crate does internally.
#[cfg(windows)]
fn connect(name: &OsStr) -> io::Result<Stream> {
    use std::{fs::OpenOptions, os::windows::ffi::OsStrExt};

    use winapi::um::namedpipeapi::WaitNamedPipeW;

    // ERROR_PIPE_BUSY — see WinError.h. `std::io::Error` does not expose a
    // typed constant for this, so the raw OS code is the cleanest test.
    const ERROR_PIPE_BUSY: i32 = 231;
    // NMPWAIT_WAIT_FOREVER — see WinBase.h. winapi 0.3 doesn't define the
    // NMPWAIT_* constants yet (only the comment placeholder).
    const NMPWAIT_WAIT_FOREVER: u32 = 0xFFFF_FFFF;

    // `WaitNamedPipeW` needs a NUL-terminated UTF-16 path.
    let mut wide: Vec<u16> = name.encode_wide().collect();
    wide.push(0);

    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Ok(file) => return Ok(file),
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                // SAFETY: `wide` is NUL-terminated; pointer stays valid for
                // the call's duration. `NMPWAIT_WAIT_FOREVER` makes this a
                // bounded kernel wait (server's pipe wait-timeout is the
                // upper bound on each retry; default ~50ms, then we loop).
                let ok = unsafe { WaitNamedPipeW(wide.as_ptr(), NMPWAIT_WAIT_FOREVER) };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                // Loop and re-open — another client may have raced us to the
                // newly-available instance.
            }
            Err(err) => return Err(err),
        }
    }
}

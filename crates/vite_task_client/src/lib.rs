use std::{
    cell::RefCell,
    ffi::OsStr,
    io::{self, Read, Write},
    sync::Arc,
};

use native_str::NativeStr;
use vite_task_ipc_shared::{GetEnvResponse, IPC_ENV_NAME, Request};
use wincode::{SchemaRead, config::DefaultConfig};

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::fs::File;

pub struct Client {
    stream: RefCell<Stream>,
    scratch: RefCell<Vec<u8>>,
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
        Self { stream: RefCell::new(stream), scratch: RefCell::new(Vec::new()) }
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

    /// Requests an env value from the runner. Returns `None` if the runner
    /// reports the env is not available.
    ///
    /// # Errors
    ///
    /// Returns an error if the request or response fails.
    // TODO(env-track): A later PR in this stack adds a tracked flag so env
    // reads can participate in cache fingerprints instead of being IPC-only.
    pub fn get_env(&self, name: &OsStr) -> io::Result<Option<Arc<OsStr>>> {
        let name = Box::<NativeStr>::from(name);

        self.send(&Request::GetEnv { name: &name })?;
        let response: GetEnvResponse = self.recv()?;
        Ok(response
            .env_value
            .map(|env_value| Arc::<OsStr>::from(env_value.to_cow_os_str().as_ref())))
    }

    fn send(&self, request: &Request<'_>) -> io::Result<()> {
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

    fn recv<T>(&self) -> io::Result<T>
    where
        for<'de> T: SchemaRead<'de, DefaultConfig, Dst = T>,
    {
        let mut stream = self.stream.borrow_mut();
        let mut scratch = self.scratch.borrow_mut();
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes)?;
        let len = u32::from_le_bytes(len_bytes) as usize;
        scratch.clear();
        scratch.resize(len, 0);
        stream.read_exact(&mut scratch)?;
        wincode::deserialize_exact(&scratch)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
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

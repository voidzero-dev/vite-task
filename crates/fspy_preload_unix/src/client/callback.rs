//! Traced-process side of the blocking open/close callback channel.
//!
//! For each matching open/close, the traced process connects to the
//! supervisor's Unix-domain socket, passes the file descriptor via
//! `SCM_RIGHTS`, sends a length-prefixed [`CallbackRequest`], and blocks until
//! the supervisor writes back a single [`CALLBACK_ACK`] byte.

use std::{
    cell::Cell,
    ffi::OsStr,
    io::{self, Read as _, Write as _},
    os::unix::{ffi::OsStrExt as _, net::UnixStream},
    path::PathBuf,
};

use bstr::BStr;
use fspy_shared::{
    callback::{CALLBACK_ACK, CallbackKind, CallbackRequest},
    ipc::{AccessMode, NativePath},
};
use passfd::FdPassingExt as _;
use wincode::Serialize as _;

use crate::libc::c_int;

/// Endpoint and access-mode mask for the blocking callback channel.
pub struct CallbackChannel {
    socket_path: PathBuf,
    mask: AccessMode,
}

thread_local! {
    /// Set while the calling thread is inside a callback round-trip, so that
    /// the open/close interception does not recurse into itself.
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// RAII reentrancy guard for the current thread.
struct ReentryGuard;

impl ReentryGuard {
    /// Returns `None` if the thread is already inside a callback round-trip.
    fn enter() -> Option<Self> {
        IN_CALLBACK
            .try_with(|flag| {
                if flag.get() {
                    None
                } else {
                    flag.set(true);
                    Some(Self)
                }
            })
            .ok()
            .flatten()
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        let _ = IN_CALLBACK.try_with(|flag| flag.set(false));
    }
}

/// Whether the calling thread is currently inside a callback round-trip.
pub fn is_reentrant() -> bool {
    IN_CALLBACK.try_with(Cell::get).unwrap_or(true)
}

/// Paths that are never worth a callback round-trip (mirrors the shared-memory
/// event filter in [`super::Client::send`]).
fn is_ignored_path(bytes: &[u8]) -> bool {
    bytes.starts_with(b"/dev/")
        || (cfg!(target_os = "linux")
            && (bytes.starts_with(b"/proc/") || bytes.starts_with(b"/sys/")))
}

impl CallbackChannel {
    // Only constructed from the decoded payload in `Client::from_env`, which
    // is itself excluded from the crate's `cfg(test)` build.
    #[cfg(not(test))]
    pub const fn new(socket_path: PathBuf, mask: AccessMode) -> Self {
        Self { socket_path, mask }
    }

    /// The access-mode mask: events fire the callback only if their mode
    /// intersects it.
    pub const fn mask(&self) -> AccessMode {
        self.mask
    }

    /// Run a blocking callback round-trip for an open/close event.
    ///
    /// Best-effort: the mask and ignored-path filters are applied first, and
    /// transient transport errors are reported to stderr without blocking the
    /// traced process forever.
    #[expect(
        clippy::print_stderr,
        reason = "preload library intentionally uses stderr for error reporting"
    )]
    pub fn round_trip(&self, kind: CallbackKind, fd: c_int, mode: AccessMode, path: Option<&BStr>) {
        if !mode.intersects(self.mask) {
            return;
        }
        if let Some(path) = path
            && is_ignored_path(path)
        {
            return;
        }
        let Some(_guard) = ReentryGuard::enter() else {
            return;
        };
        if let Err(err) = self.round_trip_inner(kind, fd, mode, path) {
            eprintln!("fspy: open/close callback round-trip failed: {err}");
        }
    }

    fn round_trip_inner(
        &self,
        kind: CallbackKind,
        fd: c_int,
        mode: AccessMode,
        path: Option<&BStr>,
    ) -> io::Result<()> {
        let native_path: Option<&NativePath> =
            path.map(|path| <&NativePath>::from(OsStr::from_bytes(path)));
        // SAFETY: `getpid` is always safe to call.
        let pid = u32::try_from(unsafe { libc::getpid() }).unwrap_or(0);
        let request = CallbackRequest { kind, mode, pid, path: native_path };

        let serialized_size = CallbackRequest::serialized_size(&request)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let size = usize::try_from(serialized_size).map_err(io::Error::other)?;
        let mut body = vec![0u8; size];
        CallbackRequest::serialize_into(&mut body[..], &request)
            .map_err(|err| io::Error::other(err.to_string()))?;
        let len = u32::try_from(body.len()).map_err(io::Error::other)?;

        // A fresh connection per event: it is dropped (and closed) while the
        // reentrancy guard is still held, so its own `close` does not recurse.
        let mut socket = UnixStream::connect(&self.socket_path)?;
        socket.send_fd(fd)?;
        socket.write_all(&len.to_le_bytes())?;
        socket.write_all(&body)?;
        let mut ack = [0u8; 1];
        socket.read_exact(&mut ack)?;
        if ack[0] != CALLBACK_ACK {
            return Err(io::Error::other("unexpected callback acknowledgement byte"));
        }
        Ok(())
    }
}

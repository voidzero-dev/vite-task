mod close;
mod execve;
mod getdents;
mod open;
mod stat;

use std::{
    borrow::Cow,
    ffi::{CString, OsStr, c_int},
    io,
    os::{
        fd::{AsFd as _, FromRawFd as _, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

use fspy_seccomp_unotify::{
    impl_handler,
    supervisor::handler::{
        HandlerResponse, NotifyResponse,
        arg::{CStrPtr, Caller, Fd},
    },
};
use fspy_shared::ipc::{AccessMode, PathAccess};

use crate::{
    arena::PathAccessArena,
    callback::{
        BorrowedFile, FileCallback, FileCallbackFn, FileEvent, FileEventKind, FileEventPath,
    },
};

const PATH_MAX: usize = libc::PATH_MAX as usize;

pub struct SyscallHandler {
    arena: PathAccessArena,
    path_read_buf: [u8; PATH_MAX],
    /// Present only when a blocking open/close callback is registered.
    callback: Option<FileCallback>,
    /// Response for the notification most recently passed to `handle_notify`.
    pending_response: NotifyResponse,
}

impl Default for SyscallHandler {
    fn default() -> Self {
        Self::with_callback(None)
    }
}

impl HandlerResponse for SyscallHandler {
    fn take_response(&mut self) -> NotifyResponse {
        std::mem::take(&mut self.pending_response)
    }
}

impl SyscallHandler {
    /// Creates a handler, optionally wired to a blocking open/close callback.
    pub fn with_callback(callback: Option<FileCallback>) -> Self {
        Self {
            arena: PathAccessArena::default(),
            path_read_buf: [0; PATH_MAX],
            callback,
            pending_response: NotifyResponse::Continue,
        }
    }

    pub fn into_arena(self) -> PathAccessArena {
        self.arena
    }

    /// Records an access to the file named by an `open`/`openat`/`execve`
    /// notification. When `is_open_syscall` is set (i.e. the syscall is a real
    /// `open*` that returns a descriptor, not an `execve`), the optional
    /// blocking callback also runs.
    fn handle_open(
        &mut self,
        caller: Caller,
        dir_fd: Fd,
        path_ptr: CStrPtr,
        flags: c_int,
        is_open_syscall: bool,
    ) -> io::Result<()> {
        let Some(path_len) = path_ptr.read(caller, &mut self.path_read_buf)? else {
            // Ignore paths that are too long to fit in PATH_MAX
            return Ok(());
        };
        let mut path = Cow::Borrowed(Path::new(OsStr::from_bytes(&self.path_read_buf[..path_len])));
        if !path.is_absolute() {
            let mut resolved_path = PathBuf::from(dir_fd.get_path(caller)?);
            if !nix::NixPath::is_empty(path.as_ref()) {
                resolved_path.push(&path);
            }
            path = Cow::Owned(resolved_path);
        }
        self.arena
            .add(PathAccess { mode: access_mode_from_flags(flags), path: path.as_os_str().into() });

        // Optional blocking callback: the supervisor opens the file itself and
        // installs the descriptor into the target via seccomp `ADDFD`. This
        // only applies to real `open*` syscalls — `execve` does not return a
        // descriptor and must continue unchanged.
        if is_open_syscall {
            let response = self
                .callback
                .as_ref()
                .and_then(|callback| run_open_callback(callback, caller, &path, flags));
            if let Some(response) = response {
                self.pending_response = response;
            }
        }
        Ok(())
    }

    fn handle_open_dir(&mut self, caller: Caller, fd: Fd) -> io::Result<()> {
        let path = fd.get_path(caller)?;
        self.arena.add(PathAccess {
            mode: AccessMode::READ_DIR,
            path: OsStr::from_bytes(path.as_bytes()).into(),
        });
        Ok(())
    }
}

/// Paths that are never worth a blocking callback round-trip.
pub(super) fn is_ignored_callback_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    bytes.starts_with(b"/dev/") || bytes.starts_with(b"/proc/") || bytes.starts_with(b"/sys/")
}

/// Derives an [`AccessMode`] from raw open `flags`.
pub(super) fn access_mode_from_flags(flags: c_int) -> AccessMode {
    match flags & libc::O_ACCMODE {
        libc::O_RDWR => AccessMode::READ | AccessMode::WRITE,
        libc::O_WRONLY => AccessMode::WRITE,
        _ => AccessMode::READ,
    }
}

/// Opens `path` in the supervisor process so the open/close callback receives
/// a usable descriptor and (for opens) the target can be handed it via `ADDFD`.
pub(super) fn open_in_supervisor(path: &Path, flags: c_int) -> io::Result<OwnedFd> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `c_path` is a valid NUL-terminated string; the mode argument is
    // only consulted by the kernel when `O_CREAT`/`O_TMPFILE` is set.
    let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o666 as libc::c_uint) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` returned a valid, owned file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Invokes the user callback for an event borrowing `fd`.
pub(super) fn invoke_callback(
    callback: &FileCallback,
    kind: FileEventKind,
    caller: Caller<'_>,
    mode: AccessMode,
    path: FileEventPath<'_>,
    fd: &OwnedFd,
) {
    let event = FileEvent {
        kind,
        pid: u32::try_from(caller.pid()).unwrap_or(0),
        mode,
        path,
        fd: BorrowedFile::new(fd.as_fd()),
    };
    let invoke: &FileCallbackFn = &*callback.callback;
    invoke(event);
}

/// Runs the blocking post-open callback and, on success, yields a response
/// that installs the supervisor's descriptor into the target.
fn run_open_callback(
    callback: &FileCallback,
    caller: Caller<'_>,
    path: &Path,
    flags: c_int,
) -> Option<NotifyResponse> {
    let mode = access_mode_from_flags(flags);
    if !mode.intersects(callback.mask) || is_ignored_callback_path(path) {
        return None;
    }
    // If the supervisor cannot open the file, fall through to `Continue` so the
    // target's own `open` runs and fails with the correct errno.
    let owned_fd = open_in_supervisor(path, flags).ok()?;
    invoke_callback(
        callback,
        FileEventKind::Opened,
        caller,
        mode,
        FileEventPath::Open(path),
        &owned_fd,
    );
    Some(NotifyResponse::ReturnFd { fd: owned_fd, cloexec: flags & libc::O_CLOEXEC != 0 })
}

impl_handler!(
    SyscallHandler:

    #[cfg(target_arch = "x86_64")] open,
    openat,
    openat2,

    close,

    #[cfg(target_arch = "x86_64")] getdents,
    getdents64,

    #[cfg(target_arch = "x86_64")] stat,
    #[cfg(target_arch = "x86_64")] lstat,
    #[cfg(target_arch = "x86_64")] newfstatat,
    #[cfg(target_arch = "aarch64")] fstatat,
    statx,

    #[cfg(target_arch = "x86_64")] access,
    faccessat,
    faccessat2,

    execve,
    execveat,
);

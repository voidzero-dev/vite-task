use std::sync::OnceLock;

use allocator_api2::alloc::Allocator;
use bstr::BStr;
use fspy_nostd::{BorrowedFd, CStr, CWD, Error, Result, Thin};
use fspy_nostd_alloc::fs::{fd_path, resolve_path_at};
use fspy_shared::ipc::{AccessMode, IpcPath, channel::Sender};
use fspy_shared_unix::{Exec, RawExec, macos::EncodedPayload};
use libc::{c_char, c_int};

use crate::which;

pub struct Client<'a> {
    encoded_payload: EncodedPayload<'a>,
    ipc_sender: Option<Sender>,
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client<'static>>();
};

#[cfg(not(test))]
impl<'a> Client<'a> {
    fn from_env(
        envs: impl Iterator<Item = fspy_nostd::env::Entry>,
        allocator: impl Allocator + Clone + 'a,
    ) -> Self {
        let encoded_payload = EncodedPayload::from_env(envs, allocator.clone());
        let ipc_sender = encoded_payload.ipc_channel_conf().sender(allocator);
        Self { encoded_payload, ipc_sender }
    }
}

impl Client<'_> {
    pub fn send_path(&self, mode: AccessMode, path: &BStr) {
        let Some(ipc_sender) = &self.ipc_sender else {
            return;
        };
        let bytes: &[u8] = path.as_ref();
        if bytes.starts_with(b"/dev/") {
            return;
        }
        ipc_sender.send(&mode.with_path(IpcPath::from_bytes(bytes)));
    }

    /// Resolves and prepares an exec call, optionally searching `PATH` first.
    ///
    /// # Safety
    ///
    /// `raw_exec` must contain valid pointers satisfying [`RawExec::to_exec`].
    /// The returned [`RawExec`] borrows from `allocator`.
    ///
    /// # Errors
    ///
    /// Returns OS-compatible `errno` values from `PATH` lookup or exec
    /// preparation.
    pub unsafe fn handle_exec<A: Allocator>(
        &self,
        search_path: bool,
        raw_exec: RawExec,
        allocator: &A,
    ) -> Result<RawExec> {
        // SAFETY: caller upholds `RawExec::to_exec`'s pointer contract.
        unsafe { self.handle_exec_in_dir(search_path, None, raw_exec, allocator) }
    }

    /// Resolves and prepares an exec call, resolving relative paths against
    /// `cwd` (when `Some`, e.g. from `posix_spawn_file_actions_addchdir_np`)
    /// instead of the current working directory.
    ///
    /// # Safety
    ///
    /// `raw_exec` must contain valid pointers satisfying [`RawExec::to_exec`].
    /// The returned [`RawExec`] borrows from `allocator`.
    ///
    /// # Errors
    ///
    /// Returns OS-compatible `errno` values from `PATH` lookup or exec
    /// preparation.
    pub unsafe fn handle_exec_in_dir<A: Allocator>(
        &self,
        search_path: bool,
        cwd: Option<&[u8]>,
        raw_exec: RawExec,
        allocator: &A,
    ) -> Result<RawExec> {
        // SAFETY: caller upholds `RawExec::to_exec`'s pointer contract.
        let mut exec = unsafe { raw_exec.to_exec(allocator) }?;
        if search_path {
            exec.program = which::resolve_in_path(&exec.program, cwd, allocator, |candidate| {
                self.send_path(AccessMode::Read, candidate);
            })?;
        }
        self.encoded_payload.prepare_exec_in_dir(&mut exec, cwd, |path| {
            self.send_path(AccessMode::Read, path);
        })?;
        Ok(RawExec::from_exec(exec))
    }

    /// Injects `DYLD_INSERT_LIBRARIES` and `FSPY_PAYLOAD` into `exec.envs`
    /// without path resolution, shebang parsing, or SIP swapping.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NOMEM`] if environment storage cannot be allocated.
    pub fn inject_env<A: Allocator + Clone>(&self, exec: &mut Exec<A>) -> Result<()> {
        self.encoded_payload.inject_env(exec)
    }
}

static CLIENT: OnceLock<Client<'static>> = OnceLock::new();

pub fn global_client() -> Option<&'static Client<'static>> {
    CLIENT.get()
}

pub fn set_errno(error: Error) {
    // SAFETY: `__error()` returns a non-null pointer to thread-local `errno`.
    unsafe { *::libc::__error() = error.raw_os_error() };
}

pub const fn open_flags_mode(flags: c_int) -> AccessMode {
    match flags & ::libc::O_ACCMODE {
        ::libc::O_RDWR => AccessMode::ReadWrite,
        ::libc::O_WRONLY => AccessMode::Write,
        _ if flags & (::libc::O_CREAT | ::libc::O_TRUNC) != 0 => AccessMode::ReadWrite,
        _ => AccessMode::Read,
    }
}

/// Decodes a `fopen`/`freopen` mode string into an [`AccessMode`].
///
/// # Safety
///
/// `mode` must be null or point to a valid NUL-terminated C string.
pub unsafe fn mode_str_to_access_mode(mode: *const c_char) -> AccessMode {
    if mode.is_null() {
        return AccessMode::Read;
    }
    // SAFETY: caller guarantees non-null `mode` points to a valid NUL-terminated string.
    let mode_bytes = unsafe { CStr::<Thin>::from_ptr(mode.cast()) }.count().as_units();
    if mode_bytes.contains(&b'+') {
        AccessMode::ReadWrite
    } else if mode_bytes.contains(&b'w') || mode_bytes.contains(&b'a') {
        AccessMode::Write
    } else {
        AccessMode::Read
    }
}

/// Reports a path relative to `CWD`.
///
/// # Safety
///
/// `path` must point to a valid NUL-terminated C string.
pub unsafe fn handle_open_path(path: *const c_char, mode: AccessMode) {
    // SAFETY: `CWD` is permanent and `path` is guaranteed valid by the caller.
    unsafe { handle_open_at(CWD.as_raw_fd(), path, mode) };
}

/// Reports a path relative to `dirfd` (or ignored when `path` is absolute).
///
/// # Safety
///
/// `dirfd` must be `-1` or an open descriptor, and `path` must point to a valid
/// NUL-terminated C string.
pub unsafe fn handle_open_at(dirfd: c_int, path: *const c_char, mode: AccessMode) {
    let Some(client) = global_client() else {
        return;
    };
    if path.is_null() {
        return;
    }
    // SAFETY: non-null `path` is NUL-terminated for the duration of this call.
    let c_path = unsafe { CStr::<Thin>::from_ptr(path.cast()) };
    if c_path.count().as_units().is_empty() {
        return;
    }
    let arena = fspy_nostd_alloc::pooled_bump();
    // SAFETY: `dirfd` is `-1` or open, and `path` is a non-null C string.
    let dirfd = unsafe { BorrowedFd::try_borrow_raw(dirfd) };
    if let Ok(Some(resolved)) = resolve_path_at(&arena, dirfd, c_path) {
        client.send_path(mode, BStr::new(resolved.as_units()));
    }
}

/// Reports an access on an open descriptor (such as `fdopendir`).
///
/// # Safety
///
/// `fd` must be `-1` or an open descriptor.
pub unsafe fn handle_open_fd(fd: c_int, mode: AccessMode) {
    let Some(client) = global_client() else {
        return;
    };
    // SAFETY: `fd` is `-1` or an open descriptor.
    let Some(fd) = (unsafe { BorrowedFd::try_borrow_raw(fd) }) else {
        return;
    };
    let arena = fspy_nostd_alloc::pooled_bump();
    if let Ok(Some(resolved)) = fd_path(&arena, fd) {
        client.send_path(mode, BStr::new(resolved.as_units()));
    }
}

#[cfg(not(test))]
#[ctor::ctor(unsafe)]
fn init_client() {
    // SAFETY: the ctor only reads the process environment while constructing
    // the client and does not retain borrowed environment views.
    let current = unsafe { fspy_nostd::env::current() }.unwrap();
    static BUMP: static_cell::StaticCell<fspy_nostd_alloc::PageBump> =
        static_cell::StaticCell::new();
    let bump: &'static fspy_nostd_alloc::PageBump = BUMP.init(fspy_nostd_alloc::page_bump());
    let client = Client::from_env(current.envs(), bump);
    assert!(CLIENT.set(client).is_ok(), "client already initialized");
}

use std::ffi::CStr;

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::{BStr, ByteSlice};
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};
use sigsafe::{AsRawFd as _, BorrowedFd, CWD};

#[cfg(target_os = "linux")]
fn get_fd_path<A: Allocator>(allocator: A, fd: BorrowedFd<'_>) -> nix::Result<Option<Vec<u8, A>>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        let path = sigsafe_alloc::fs::getcwd(allocator)
            .map_err(|errno| nix::errno::Errno::from_raw(errno.raw_os_error()))?
            .into_bytes();
        return Ok(Some(path));
    }
    let mut path = [0; PROC_FD_PATH_CAPACITY];
    let path = proc_fd_path(fd, &mut path);
    match sigsafe_alloc::fs::readlinkat(allocator, CWD, path) {
        Ok(path) => Ok(Some(path)),
        Err(sigsafe::Errno::BADF | sigsafe::Errno::NOENT) => Ok(None),
        Err(errno) => Err(nix::errno::Errno::from_raw(errno.raw_os_error())),
    }
}

#[cfg(target_os = "linux")]
const PROC_FD_PATH_CAPACITY: usize = b"/proc/self/fd/".len() + 11 + 1;

#[cfg(target_os = "linux")]
fn proc_fd_path<'buf>(
    fd: BorrowedFd<'_>,
    buf: &'buf mut [u8; PROC_FD_PATH_CAPACITY],
) -> sigsafe::CStr<'buf, sigsafe::Thin> {
    const PREFIX: &[u8] = b"/proc/self/fd/";

    let mut formatted = itoa::Buffer::new();
    let fd = formatted.format(fd.as_raw_fd()).as_bytes();

    buf[..PREFIX.len()].copy_from_slice(PREFIX);
    let end = PREFIX.len() + fd.len();
    buf[PREFIX.len()..end].copy_from_slice(fd);
    buf[end] = 0;

    // SAFETY: the initialized prefix ends in NUL and lives as long as `buf`.
    unsafe { sigsafe::CStr::from_ptr(buf.as_ptr().cast()) }
}

#[cfg(target_os = "macos")]
fn get_fd_path<A: Allocator>(allocator: A, fd: BorrowedFd<'_>) -> nix::Result<Option<Vec<u8, A>>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        let path = sigsafe_alloc::fs::getcwd(allocator)
            .map_err(|errno| nix::errno::Errno::from_raw(errno.raw_os_error()))?
            .into_bytes();
        return Ok(Some(path));
    }

    match sigsafe_alloc::fs::fcntl_getpath(allocator, fd) {
        Ok(path) => {
            // `F_GETPATH` does not return a length. Count at this caller before
            // converting its allocation into the returned path.
            Ok(Some(path.count().into_bytes()))
        }
        Err(sigsafe::Errno::BADF | sigsafe::Errno::NOENT) => Ok(None),
        Err(errno) => Err(nix::errno::Errno::from_raw(errno.raw_os_error())),
    }
}

pub trait ToAbsolutePath {
    fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R>;
}

impl ToAbsolutePath for BorrowedFd<'_> {
    fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        let arena = sigsafe_alloc::arena();
        let path = get_fd_path(&arena, self)?;
        f(path.as_ref().map(|path| path.as_slice().as_bstr()))
    }
}

pub struct PathAt<'fd, 'path>(pub BorrowedFd<'fd>, pub sigsafe::CStr<'path, sigsafe::Thin>);

impl PathAt<'_, '_> {
    /// Borrows raw directory-descriptor and pathname arguments.
    ///
    /// # Safety
    ///
    /// `fd` must remain valid while the returned value is used, and `path`
    /// must point to a valid NUL-terminated string.
    pub const unsafe fn borrow_raw(fd: c_int, path: *const c_char) -> Self {
        // SAFETY: both invariants are upheld by the caller.
        Self(unsafe { BorrowedFd::borrow_raw(fd) }, unsafe { sigsafe::CStr::from_ptr(path) })
    }
}

impl ToAbsolutePath for PathAt<'_, '_> {
    fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        let pathname = self.1.count().as_bytes().as_bstr();

        if pathname.starts_with(b"/") {
            f(Some(pathname))
        } else {
            self.0.to_absolute_path(|base| {
                let Some(base) = base else {
                    return f(None);
                };
                if pathname.is_empty() {
                    return f(Some(base));
                }

                let arena = sigsafe_alloc::arena();
                let needs_separator = !base.ends_with(b"/");
                let mut abs_path = Vec::with_capacity_in(
                    base.len() + usize::from(needs_separator) + pathname.len(),
                    &arena,
                );
                abs_path.extend_from_slice(base);
                if needs_separator {
                    abs_path.push(b'/');
                }
                abs_path.extend_from_slice(pathname);
                f(Some(abs_path.as_slice().as_bstr()))
            })
        }
    }
}

impl ToAbsolutePath for sigsafe::CStr<'_, sigsafe::Thin> {
    fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        PathAt(CWD, self).to_absolute_path(f)
    }
}

pub trait ToAccessMode {
    unsafe fn to_access_mode(self) -> AccessMode;
}

impl ToAccessMode for AccessMode {
    unsafe fn to_access_mode(self) -> AccessMode {
        self
    }
}

pub struct OpenFlags(pub c_int);
impl ToAccessMode for OpenFlags {
    unsafe fn to_access_mode(self) -> AccessMode {
        match self.0 & libc::O_ACCMODE {
            libc::O_RDWR => AccessMode::READ | AccessMode::WRITE,
            libc::O_WRONLY => AccessMode::WRITE,
            _ => AccessMode::READ,
        }
    }
}

pub struct ModeStr(pub *const c_char);
impl ToAccessMode for ModeStr {
    unsafe fn to_access_mode(self) -> AccessMode {
        // SAFETY: self.0 is a non-null pointer to a valid null-terminated C string, as guaranteed by the libc calling convention
        let mode_str = unsafe { CStr::from_ptr(self.0) }.to_bytes().as_bstr();
        let has_read = mode_str.contains(&b'r');
        let has_write = mode_str.contains(&b'w') || mode_str.contains(&b'a');
        match (has_read, has_write) {
            (false, true) => AccessMode::WRITE,
            (true, true) => AccessMode::READ | AccessMode::WRITE,
            _ => AccessMode::READ,
        }
    }
}

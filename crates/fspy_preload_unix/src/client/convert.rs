#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::{
    ffi::{CStr, OsStr},
    os::unix::ffi::OsStrExt as _,
    path::PathBuf,
};

use bstr::{BStr, ByteSlice};
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};
use nix::unistd::getcwd;
use sigsafe::{AsRawFd as _, BorrowedFd, CWD};

#[cfg(target_os = "linux")]
fn get_fd_path(fd: BorrowedFd<'_>) -> nix::Result<Option<PathBuf>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        return Ok(Some(getcwd()?));
    }
    match nix::fcntl::readlink(
        CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).unwrap().as_c_str(),
    ) {
        Ok(path) => Ok(Some(path.into())),
        Err(nix::Error::EBADF | nix::Error::ENOENT) => Ok(None), // invalid fd or no such file (Most likely a stdio fd)
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "macos")]
fn get_fd_path(fd: BorrowedFd<'_>) -> nix::Result<Option<PathBuf>> {
    if fd.as_raw_fd() == CWD.as_raw_fd() {
        return Ok(Some(getcwd()?));
    }
    let mut path = std::path::PathBuf::new();
    // SAFETY: this std view has the same descriptor and lifetime as the
    // rustix view accepted by this function.
    let fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd.as_raw_fd()) };
    match nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut path)) {
        Ok(_) => Ok(Some(path)),
        Err(nix::Error::EBADF | nix::Error::ENOENT) => Ok(None), // invalid fd or no such file (Most likely a stdio fd)
        Err(e) => Err(e),
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
        let path = get_fd_path(self)?;
        f(path.as_ref().map(|path| path.as_os_str().as_bytes().as_bstr()))
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
        // SAFETY: self.1 is a valid NUL-terminated string.
        let pathname = unsafe { CStr::from_ptr(self.1.as_ptr()) }.to_bytes().as_bstr();

        if pathname.first().copied() == Some(b'/') {
            f(pathname.into())
        } else {
            let Some(mut abs_path) = get_fd_path(self.0)? else {
                return f(None);
            };
            if !pathname.is_empty() {
                abs_path.push(OsStr::from_bytes(pathname));
            }
            f(Some(abs_path.as_os_str().as_bytes().as_bstr()))
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

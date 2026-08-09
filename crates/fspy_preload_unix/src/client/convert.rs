#[cfg(target_os = "linux")]
use std::ffi::CString;
use std::{
    ffi::{CStr, OsStr},
    os::{fd::RawFd, unix::ffi::OsStrExt as _},
    path::PathBuf,
};

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::{BStr, ByteSlice};
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

#[cfg(target_os = "linux")]
fn get_fd_path(fd: RawFd) -> nix::Result<Option<PathBuf>> {
    match nix::fcntl::readlink(CString::new(format!("/proc/self/fd/{fd}")).unwrap().as_c_str()) {
        Ok(path) => Ok(Some(path.into())),
        Err(nix::Error::EBADF | nix::Error::ENOENT) => Ok(None), // invalid fd or no such file (Most likely a stdio fd)
        Err(e) => Err(e),
    }
}

#[cfg(target_os = "macos")]
fn get_fd_path(fd: RawFd) -> nix::Result<Option<PathBuf>> {
    let mut path = std::path::PathBuf::new();
    match nix::fcntl::fcntl(
        // SAFETY: fd is a valid file descriptor provided by the caller, and the borrow does not outlive this function call
        unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) },
        nix::fcntl::FcntlArg::F_GETPATH(&mut path),
    ) {
        Ok(_) => Ok(Some(path)),
        Err(nix::Error::EBADF | nix::Error::ENOENT) => Ok(None), // invalid fd or no such file (Most likely a stdio fd)
        Err(e) => Err(e),
    }
}

fn getcwd<A: Allocator>(allocator: A) -> nix::Result<Vec<u8, A>> {
    let bytes = sigsafe_alloc::fs::getcwd(allocator)
        .map_err(|errno| nix::errno::Errno::from_raw(errno.raw_os_error()))?
        .into_bytes();
    Ok(bytes)
}
pub trait ToAbsolutePath {
    unsafe fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R>;
}

pub struct Fd(pub c_int);
impl ToAbsolutePath for Fd {
    unsafe fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        if self.0 == libc::AT_FDCWD {
            let arena = sigsafe_alloc::arena();
            let path = getcwd(&arena)?;
            return f(Some(path.as_slice().as_bstr()));
        }

        let path = get_fd_path(self.0)?;
        f(path.as_ref().map(|path| path.as_os_str().as_bytes().as_bstr()))
    }
}

pub struct PathAt(pub c_int, pub *const c_char);

impl ToAbsolutePath for PathAt {
    unsafe fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        // SAFETY: self.1 is a non-null pointer to a valid null-terminated C string, as guaranteed by the libc calling convention
        let pathname = unsafe { CStr::from_ptr(self.1) }.to_bytes().as_bstr();

        if pathname.first().copied() == Some(b'/') {
            f(pathname.into())
        } else if self.0 == libc::AT_FDCWD {
            let arena = sigsafe_alloc::arena();
            let mut abs_path = getcwd(&arena)?;
            if !pathname.is_empty() {
                if !abs_path.ends_with(b"/") {
                    abs_path.push(b'/');
                }
                abs_path.extend_from_slice(pathname);
            }
            f(Some(abs_path.as_slice().as_bstr()))
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

impl ToAbsolutePath for *const c_char {
    unsafe fn to_absolute_path<R, F: FnOnce(Option<&BStr>) -> nix::Result<R>>(
        self,
        f: F,
    ) -> nix::Result<R> {
        // SAFETY: delegates to PathAt::to_absolute_path with AT_FDCWD and the caller-provided C string pointer
        unsafe { PathAt(libc::AT_FDCWD, self).to_absolute_path(f) }
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

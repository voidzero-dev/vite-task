#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt as _;
use std::{ffi::CStr, os::fd::RawFd};

use allocator_api2::{alloc::Allocator, vec::Vec};
use bstr::{BStr, ByteSlice};
use fspy_shared::ipc::AccessMode;
use libc::{c_char, c_int};

#[cfg(target_os = "linux")]
fn get_fd_path<A: Allocator>(fd: RawFd, allocator: A) -> nix::Result<Option<Vec<u8, A>>> {
    let mut path = [0; PROC_FD_PATH_CAPACITY];
    let path = proc_fd_path(fd, &mut path);
    match sigsafe_alloc::fs::readlink(allocator, path) {
        Ok(path) => Ok(Some(path)),
        Err(sigsafe::Errno::BADF | sigsafe::Errno::NOENT) => Ok(None),
        Err(errno) => Err(nix::errno::Errno::from_raw(errno.raw_os_error())),
    }
}

#[cfg(target_os = "linux")]
const PROC_FD_PATH_CAPACITY: usize = b"/proc/self/fd/".len() + 11 + 1;

#[cfg(target_os = "linux")]
fn proc_fd_path(
    fd: RawFd,
    buf: &mut [u8; PROC_FD_PATH_CAPACITY],
) -> sigsafe::CStr<'_, sigsafe::Thin> {
    const PREFIX: &[u8] = b"/proc/self/fd/";

    let mut digits = [0; 11];
    let mut start = digits.len();
    let mut value = fd.unsigned_abs();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    if fd.is_negative() {
        start -= 1;
        digits[start] = b'-';
    }

    buf[..PREFIX.len()].copy_from_slice(PREFIX);
    let end = PREFIX.len() + digits.len() - start;
    buf[PREFIX.len()..end].copy_from_slice(&digits[start..]);
    buf[end] = 0;

    // SAFETY: the initialized prefix ends in NUL and lives as long as `buf`.
    unsafe { sigsafe::CStr::from_ptr(buf.as_ptr().cast()) }
}

#[cfg(target_os = "macos")]
fn get_fd_path(fd: RawFd) -> nix::Result<Option<std::path::PathBuf>> {
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

        #[cfg(target_os = "linux")]
        {
            let arena = sigsafe_alloc::arena();
            let path = get_fd_path(self.0, &arena)?;
            f(path.as_ref().map(|path| path.as_slice().as_bstr()))
        }

        #[cfg(target_os = "macos")]
        {
            let path = get_fd_path(self.0)?;
            f(path.as_ref().map(|path| path.as_os_str().as_bytes().as_bstr()))
        }
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
        } else {
            // SAFETY: delegates the same caller-provided descriptor to Fd.
            unsafe {
                Fd(self.0).to_absolute_path(|base| {
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

//! I/O calls with caller-owned buffers.

use crate::{BorrowedFd, Error, Result};

/// Reads into `buf` with a single `read(2)` and returns the initialized byte
/// count.
///
/// # Errors
///
/// Returns the error reported by `read`.
#[cfg(any(target_os = "linux", target_os = "none"))]
pub fn read(fd: BorrowedFd<'_>, buf: &mut [u8]) -> Result<usize> {
    // SAFETY: `fd` stays borrowed and `buf` is writable for its whole length.
    unsafe {
        syscalls::syscall!(syscalls::Sysno::read, fd.as_raw_fd(), buf.as_mut_ptr(), buf.len())
    }
    .map_err(Error::from)
}

/// Writes `buf` to `fd` with a single `write(2)` and returns the number of
/// bytes the kernel accepted.
///
/// A short write is reported as-is, exactly like the syscall; the caller
/// decides whether to write the remainder.
///
/// # Errors
///
/// Returns the error reported by `write`.
pub fn write(fd: BorrowedFd<'_>, buf: &[u8]) -> Result<usize> {
    #[cfg(any(target_os = "linux", target_os = "none"))]
    {
        // SAFETY: `fd` stays borrowed and `buf` is readable for its full length.
        unsafe {
            syscalls::syscall!(syscalls::Sysno::write, fd.as_raw_fd(), buf.as_ptr(), buf.len())
        }
        .map_err(Error::from)
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `fd` stays borrowed and `buf` is readable for its full length.
        let written = unsafe { libc::write(fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
        if written == -1 { Err(Error::last_os_error()) } else { Ok(written.cast_unsigned()) }
    }
}

//! I/O calls with caller-owned buffers.
//!
//! Linux only: these go straight to the kernel with no libc wrapper, so they
//! are usable from a signal handler, a post-`fork()` child, or freestanding
//! injected code.

use crate::{BorrowedFd, Error, Result};

/// Reads into `buf` with a single `read(2)` and returns the initialized byte
/// count.
///
/// # Errors
///
/// Returns the error reported by `read`.
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
    // SAFETY: `fd` stays borrowed for the call and `buf` is readable for its
    // whole length. The kernel receives the descriptor, buffer pointer, and
    // length explicitly and returns the accepted byte count.
    let written = unsafe {
        syscalls::syscall!(syscalls::Sysno::write, fd.as_raw_fd(), buf.as_ptr(), buf.len())
    }
    .map_err(Error::from)?;
    Ok(written)
}

use core::{mem::MaybeUninit, slice};

use rustix::fd::FromRawFd as _;

use crate::{
    AsRawFd as _, BorrowedFd, CStr, Error, Fat, OwnedFd, Result, Thin,
    fs::{AtFlags, Mode, OFlags},
};

// Linux UAPI `PATH_MAX`.
pub(super) const PATH_MAX: usize = 4096;

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub(super) fn openat<R>(
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, R>,
    flags: OFlags,
    mode: Mode,
) -> Result<OwnedFd> {
    // SAFETY: `dirfd` remains borrowed and `path` is NUL-terminated. The
    // kernel receives all four syscall arguments explicitly.
    let fd = unsafe {
        syscalls::syscall!(
            syscalls::Sysno::openat,
            dirfd.as_raw_fd(),
            path.as_ptr(),
            flags.bits(),
            mode.bits()
        )
    }
    .map_err(|errno| Error::from_raw_os_error(errno.into_raw()))?;

    // This should not fail with a well-behaved kernel: `openat` returns a
    // nonnegative `c_int` file descriptor.
    let fd = i32::try_from(fd).map_err(|_| Error::OVERFLOW)?;

    // SAFETY: a successful `openat` returns a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[expect(clippy::needless_pass_by_value, reason = "CStr is a borrowed value type")]
pub(super) fn unlinkat<R>(dirfd: BorrowedFd<'_>, path: CStr<'_, R>, flags: AtFlags) -> Result<()> {
    // SAFETY: `dirfd` remains borrowed and `path` is NUL-terminated for the
    // syscall.
    unsafe {
        syscalls::syscall!(
            syscalls::Sysno::unlinkat,
            dirfd.as_raw_fd(),
            path.as_ptr(),
            flags.bits()
        )
    }
    .map_err(|errno| Error::from_raw_os_error(errno.into_raw()))?;
    Ok(())
}

/// Reads the target of `path` relative to `dirfd` into `buf`.
///
/// The returned bytes borrow the initialized prefix of `buf`. As with the
/// underlying syscall, they do not include a terminating NUL, and a result
/// that fills `buf` may have been truncated.
///
/// This function performs one syscall and does not retry a full buffer.
///
/// # Errors
///
/// Returns the error reported by `readlinkat`.
pub fn readlinkat<'buf>(
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, Thin>,
    buf: &'buf mut [MaybeUninit<u8>],
) -> Result<&'buf [u8]> {
    // SAFETY: `dirfd` remains borrowed, `path` is NUL-terminated, and `buf`
    // is writable for `buf.len()` bytes. `readlinkat` returns the initialized
    // byte count.
    let initialized = unsafe {
        syscalls::syscall!(
            syscalls::Sysno::readlinkat,
            dirfd.as_raw_fd(),
            path.as_ptr(),
            buf.as_mut_ptr(),
            buf.len()
        )
    }
    .map_err(|errno| Error::from_raw_os_error(errno.into_raw()))?;

    // SAFETY: the syscall initialized exactly this prefix.
    Ok(unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) })
}

pub(super) fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    // rustix exposes only an allocating `getcwd`, so use the raw syscall for
    // caller-owned storage.
    // SAFETY: `buf` is writable for exactly `buf.len()` bytes. The syscall
    // writes no more than that and returns the initialized length including
    // its terminating NUL.
    let initialized =
        unsafe { syscalls::syscall!(syscalls::Sysno::getcwd, buf.as_mut_ptr(), buf.len()) }
            .map_err(|errno| Error::from_raw_os_error(errno.into_raw()))?;

    // SAFETY: the syscall initialized this prefix through its terminating NUL.
    let bytes = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) };
    // SAFETY: the syscall returned one NUL-terminated pathname.
    Ok(unsafe { CStr::from_units_with_nul_unchecked(bytes) })
}

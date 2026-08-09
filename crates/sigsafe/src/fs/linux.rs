use core::{mem::MaybeUninit, slice};

use crate::{CStr, Errno, Fat, Result, Thin};

// Linux UAPI `PATH_MAX`.
pub(super) const PATH_MAX: usize = 4096;
const AT_FDCWD: usize = (-100_isize).cast_unsigned();

/// Reads the target of `path` into `buf`.
///
/// The returned bytes borrow the initialized prefix of `buf`. As with the
/// underlying syscall, they do not include a terminating NUL, and a result
/// that fills `buf` may have been truncated.
///
/// # Errors
///
/// Returns the error reported by `readlinkat`.
pub fn readlink<'buf>(
    path: CStr<'_, Thin>,
    buf: &'buf mut [MaybeUninit<u8>],
) -> Result<&'buf [u8]> {
    // SAFETY: `path` is NUL-terminated and `buf` is writable for `buf.len()`
    // bytes. `readlinkat` returns the initialized byte count.
    let initialized = unsafe {
        syscalls::syscall4(
            syscalls::Sysno::readlinkat,
            AT_FDCWD,
            path.as_ptr().addr(),
            buf.as_mut_ptr().addr(),
            buf.len(),
        )
    }
    .map_err(|errno| Errno::from_raw_os_error(errno.into_raw()))?;

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
        unsafe { syscalls::syscall2(syscalls::Sysno::getcwd, buf.as_mut_ptr().addr(), buf.len()) }
            .map_err(|errno| Errno::from_raw_os_error(errno.into_raw()))?;

    // SAFETY: the syscall initialized this prefix through its terminating NUL.
    let bytes = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) };
    // SAFETY: the syscall returned one NUL-terminated pathname.
    Ok(unsafe { CStr::from_bytes_with_nul_unchecked(bytes) })
}

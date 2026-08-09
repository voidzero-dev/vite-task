use core::{mem::MaybeUninit, slice};

use crate::{CStr, Errno, Fat, Result};

// Linux UAPI `PATH_MAX`.
pub(super) const PATH_MAX: usize = 4096;

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

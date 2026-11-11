use core::{mem::MaybeUninit, slice};

use crate::{
    BorrowedFd, CStr, Error, Fat, OwnedFd, Result, Thin,
    fs::{AtFlags, Mode, OFlags, Stat},
};

// Linux UAPI `PATH_MAX`.
pub(super) const PATH_MAX: usize = 4096;
pub(super) const O_RDONLY: u32 = linux_raw_sys::general::O_RDONLY;
pub(super) const O_RDWR: u32 = linux_raw_sys::general::O_RDWR;
pub(super) const O_CREAT: u32 = linux_raw_sys::general::O_CREAT;
pub(super) const O_EXCL: u32 = linux_raw_sys::general::O_EXCL;
pub(super) const O_CLOEXEC: u32 = linux_raw_sys::general::O_CLOEXEC;
pub(super) const AT_REMOVEDIR: u32 = linux_raw_sys::general::AT_REMOVEDIR;
pub(super) type ModeBits = u32;
pub(super) const S_IRUSR: ModeBits = linux_raw_sys::general::S_IRUSR;
pub(super) const S_IWUSR: ModeBits = linux_raw_sys::general::S_IWUSR;

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
    .map_err(Error::from)?;

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
    .map_err(Error::from)?;
    Ok(())
}

pub(super) fn fstat(fd: BorrowedFd<'_>) -> Result<Stat> {
    let mut raw = core::mem::MaybeUninit::<linux_raw_sys::general::stat>::zeroed();
    // SAFETY: `fd` remains borrowed and `raw` points to writable storage for
    // the kernel's architecture-specific `stat` structure.
    unsafe { syscalls::syscall!(syscalls::Sysno::fstat, fd.as_raw_fd(), raw.as_mut_ptr()) }
        .map_err(Error::from)?;
    // SAFETY: a successful `fstat` initialized the complete structure.
    let raw = unsafe { raw.assume_init() };
    Ok(Stat { st_size: raw.st_size })
}

pub(super) fn ftruncate(fd: BorrowedFd<'_>, len: u64) -> Result<()> {
    // SAFETY: `fd` remains borrowed; the kernel receives the desired length
    // by value and validates whether it is representable for the file.
    unsafe { syscalls::syscall!(syscalls::Sysno::ftruncate, fd.as_raw_fd(), len) }
        .map_err(Error::from)?;
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
    .map_err(Error::from)?;

    // SAFETY: the syscall initialized exactly this prefix.
    Ok(unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) })
}

pub(super) fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    // SAFETY: `buf` is writable for exactly `buf.len()` bytes. The syscall
    // writes no more than that and returns the initialized length including
    // its terminating NUL.
    let initialized =
        unsafe { syscalls::syscall!(syscalls::Sysno::getcwd, buf.as_mut_ptr(), buf.len()) }
            .map_err(Error::from)?;

    // SAFETY: the syscall initialized this prefix through its terminating NUL.
    let bytes = unsafe { slice::from_raw_parts(buf.as_ptr().cast(), initialized) };
    // SAFETY: the syscall returned one NUL-terminated pathname.
    Ok(unsafe { CStr::from_units_with_nul_unchecked(bytes) })
}

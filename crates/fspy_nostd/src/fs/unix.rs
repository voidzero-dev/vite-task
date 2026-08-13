use core::mem::MaybeUninit;

pub use rustix::fs::{AtFlags, Mode, OFlags, fstat, ftruncate};

#[cfg(target_os = "linux")]
use super::linux as imp;
#[cfg(target_os = "linux")]
pub use super::linux::readlinkat;
#[cfg(target_os = "macos")]
use super::mac as imp;
#[cfg(target_os = "macos")]
pub use super::mac::fcntl_getpath;
use crate::{BorrowedFd, CStr, Fat, OwnedFd, Result};

/// The platform's maximum pathname size, including the terminating NUL.
pub const PATH_MAX: usize = imp::PATH_MAX;

/// Opens `path` relative to `dirfd` and returns its owned descriptor.
///
/// # Errors
///
/// Returns the error reported by `openat`.
pub fn openat<R>(
    dirfd: BorrowedFd<'_>,
    path: CStr<'_, R>,
    flags: OFlags,
    mode: Mode,
) -> Result<OwnedFd> {
    imp::openat(dirfd, path, flags, mode)
}

/// Removes `path` relative to `dirfd`.
///
/// # Errors
///
/// Returns the error reported by `unlinkat`.
pub fn unlinkat<R>(dirfd: BorrowedFd<'_>, path: CStr<'_, R>, flags: AtFlags) -> Result<()> {
    imp::unlinkat(dirfd, path, flags)
}

/// Writes the absolute pathname of the current working directory into `buf`.
///
/// The returned C string borrows `buf`, starts at the same address as `buf`,
/// and includes a terminating NUL. Bytes after that terminator have an
/// unspecified initialization state.
///
/// This function performs one resolution attempt and does not allocate, grow,
/// or retry. No buffer size guarantees success, including one larger than
/// [`PATH_MAX`].
///
/// # Errors
///
/// Returns the error reported while resolving the current working directory.
pub fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    imp::getcwd(buf)
}

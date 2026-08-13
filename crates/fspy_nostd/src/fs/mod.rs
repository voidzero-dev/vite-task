//! Filesystem calls with caller-owned storage.

#[cfg(unix)]
use core::mem::MaybeUninit;

#[cfg(unix)]
pub use rustix::fs::{AtFlags, Mode, OFlags, fstat, ftruncate};

#[cfg(unix)]
use crate::{BorrowedFd, CStr, Fat, OwnedFd, Result};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{create_file, delete_file, get_file_size};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "linux")]
pub use linux::readlinkat;
#[cfg(target_os = "macos")]
use mac as imp;
#[cfg(target_os = "macos")]
pub use mac::fcntl_getpath;

/// The platform's maximum pathname size, including the terminating NUL.
#[cfg(unix)]
pub const PATH_MAX: usize = imp::PATH_MAX;

/// Opens `path` relative to `dirfd` and returns its owned descriptor.
///
/// # Errors
///
/// Returns the error reported by `openat`.
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
pub fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    imp::getcwd(buf)
}

#[cfg(all(test, unix))]
mod tests;

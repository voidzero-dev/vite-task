use core::mem::MaybeUninit;

use bitflags::bitflags;

#[cfg(any(target_os = "linux", target_os = "none"))]
use super::linux as imp;
#[cfg(any(target_os = "linux", target_os = "none"))]
pub use super::linux::readlinkat;
#[cfg(target_os = "macos")]
use super::mac as imp;
#[cfg(target_os = "macos")]
pub use super::mac::fcntl_getpath;
use crate::{BorrowedFd, CStr, Fat, OwnedFd, Result};

type ModeBits = imp::ModeBits;

bitflags! {
    /// Options accepted by file-opening operations.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct OFlags: u32 {
        /// Open for reading.
        const RDONLY = imp::O_RDONLY;
        /// Open for reading and writing.
        const RDWR = imp::O_RDWR;
        /// Create the file when it does not exist.
        const CREATE = imp::O_CREAT;
        /// Fail when creating a file that already exists.
        const EXCL = imp::O_EXCL;
        /// Close the descriptor across `exec`.
        const CLOEXEC = imp::O_CLOEXEC;
    }

    /// Permission bits used when creating a file.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Mode: ModeBits {
        /// Owner read permission.
        const RUSR = imp::S_IRUSR;
        /// Owner write permission.
        const WUSR = imp::S_IWUSR;
    }

    /// Options accepted by `*at` filesystem operations.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct AtFlags: u32 {
        /// Remove a directory rather than a non-directory entry.
        const REMOVEDIR = imp::AT_REMOVEDIR;
    }
}

/// Metadata used by fspy's shared-memory backing files.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub st_size: i64,
}

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

/// Returns metadata for an open descriptor.
///
/// # Errors
///
/// Returns the error reported by `fstat`.
pub fn fstat(fd: BorrowedFd<'_>) -> Result<Stat> {
    imp::fstat(fd)
}

/// Sets the length of an open file.
///
/// # Errors
///
/// Returns the error reported by `ftruncate`.
pub fn ftruncate(fd: BorrowedFd<'_>, len: u64) -> Result<()> {
    imp::ftruncate(fd, len)
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

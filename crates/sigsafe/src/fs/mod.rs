//! Filesystem calls with caller-owned storage.

use core::mem::MaybeUninit;

use crate::{CStr, Fat, Result};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "linux")]
pub use linux::readlink;
#[cfg(target_os = "macos")]
use mac as imp;

/// The platform's maximum pathname size, including the terminating NUL.
pub const PATH_MAX: usize = imp::PATH_MAX;

/// Writes the absolute pathname of the current working directory into `buf`.
///
/// The returned C string borrows `buf` and includes a terminating NUL. Bytes
/// after that terminator have an unspecified initialization state.
///
/// This function does not allocate, grow, or retry. No buffer size guarantees
/// success, including one larger than [`PATH_MAX`].
///
/// # Errors
///
/// Returns the error reported while resolving the current working directory.
pub fn getcwd(buf: &mut [MaybeUninit<u8>]) -> Result<CStr<'_, Fat>> {
    imp::getcwd(buf)
}

#[cfg(test)]
mod tests;

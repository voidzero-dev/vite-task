//! Borrowed standard output and standard error descriptors.
//!
//! These do not buffer output or coordinate with other writers.

use crate::BorrowedFd;

#[cfg(any(target_os = "linux", target_os = "none"))]
const STDOUT_FILENO: crate::RawFd = linux_raw_sys::general::STDOUT_FILENO.cast_signed();
#[cfg(any(target_os = "linux", target_os = "none"))]
const STDERR_FILENO: crate::RawFd = linux_raw_sys::general::STDERR_FILENO.cast_signed();
#[cfg(target_os = "macos")]
use libc::{STDERR_FILENO, STDOUT_FILENO};

/// Borrows the standard output descriptor without taking ownership.
///
/// # Safety
///
/// The descriptor must remain open for the returned borrow's lifetime; no
/// thread may close or replace it during that time.
#[inline]
#[must_use]
pub const unsafe fn stdout() -> BorrowedFd<'static> {
    // SAFETY: the caller keeps the standard output descriptor open.
    unsafe { BorrowedFd::borrow_raw(STDOUT_FILENO) }
}

/// Borrows the standard error descriptor without taking ownership.
///
/// # Safety
///
/// The descriptor must remain open for the returned borrow's lifetime; no
/// thread may close or replace it during that time.
#[inline]
#[must_use]
pub const unsafe fn stderr() -> BorrowedFd<'static> {
    // SAFETY: the caller keeps the standard error descriptor open.
    unsafe { BorrowedFd::borrow_raw(STDERR_FILENO) }
}

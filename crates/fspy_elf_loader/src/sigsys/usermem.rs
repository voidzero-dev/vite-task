//! User memory copies for the `SIGSYS` dispatcher.
//!
//! Pointers passed to trapped syscalls are assumed to be valid, as the macOS
//! and Windows preloads assume for the calls they interpose, and are read and
//! written directly. Only addresses in the first page (`NULL` and small
//! integers) are rejected with `EFAULT` without being touched.
//!
//! Any other bad pointer crashes the process where the kernel would return
//! `EFAULT`, because `HANDLER_BLOCK_MASK` blocks `SIGSEGV` and `SIGBUS` while
//! the handler runs. No known program passes one to the syscalls handled
//! here; the one known address-probe idiom, Abseil's
//! `rt_sigprocmask(~0, addr, NULL, 8)`, is passed straight to the kernel
//! without reading `addr`. Copying through `process_vm_readv` instead would
//! cost two syscalls per copy and trip target filters that trap it, such as
//! Chromium's sandbox.

use core::{mem::size_of, ptr, slice};

use fspy_nostd::{CStr, Error, Result, Thin};

/// Minimum valid user-space address (page 0 is never mapped on Linux).
const MIN_USER_ADDR: usize = 4096;

#[inline]
fn check_range(addr: usize, len: usize) -> Result<()> {
    if len == 0 {
        return Ok(());
    }
    if addr < MIN_USER_ADDR {
        return Err(Error::FAULT);
    }
    addr.checked_add(len).ok_or(Error::FAULT)?;
    Ok(())
}

/// Copies a `Copy` value of type `T` from user address `addr`, returning
/// [`Error::FAULT`] if `addr` is in the first page.
///
/// # Safety
///
/// `T` must be valid for any bit pattern (e.g. integers or `#[repr(C)]` kernel
/// UAPI structs of integers/raw pointers), and `addr` must be null, in the
/// first page, or readable for `size_of::<T>()` bytes.
pub unsafe fn read_user<T: Copy>(addr: usize) -> Result<T> {
    check_range(addr, size_of::<T>())?;
    // SAFETY: the caller guarantees `addr..addr + size_of::<T>()` is readable.
    Ok(unsafe { ptr::with_exposed_provenance::<T>(addr).read_unaligned() })
}

/// Copies a `Copy` value of type `T` to user address `addr`, returning
/// [`Error::FAULT`] if `addr` is in the first page.
///
/// # Safety
///
/// `addr` must be null, in the first page, or writable for
/// `size_of::<T>()` bytes.
pub unsafe fn write_user<T: Copy>(addr: usize, value: &T) -> Result<()> {
    // SAFETY: `value` is a valid initialized `T` of `size_of::<T>()` bytes.
    let bytes = unsafe { slice::from_raw_parts(ptr::from_ref(value).cast::<u8>(), size_of::<T>()) };
    // SAFETY: forwarded from the caller contract.
    unsafe { write_user_bytes(addr, bytes) }
}

/// Copies `src` bytes to user address `addr`, returning [`Error::FAULT`] if
/// `addr` is in the first page.
///
/// # Safety
///
/// `addr` must be null, in the first page, or writable for `src.len()`
/// bytes that do not overlap `src`.
pub unsafe fn write_user_bytes(addr: usize, src: &[u8]) -> Result<()> {
    check_range(addr, src.len())?;
    // SAFETY: the caller guarantees `addr..addr + src.len()` is writable and
    // disjoint from `src`.
    unsafe {
        ptr::copy_nonoverlapping(src.as_ptr(), ptr::with_exposed_provenance_mut(addr), src.len());
    }
    Ok(())
}

/// Borrows a NUL-terminated C string directly from user address `addr`,
/// returning [`Error::FAULT`] if `addr` is in the first page (such as `NULL`).
///
/// # Safety
///
/// Any other `addr` must point to a readable NUL-terminated C string that
/// remains valid and unmodified for `'a`.
pub unsafe fn user_cstr<'a>(addr: usize) -> Result<CStr<'a, Thin>> {
    if addr < MIN_USER_ADDR {
        return Err(Error::FAULT);
    }
    // SAFETY: caller guarantees `addr` points to a readable NUL-terminated C
    // string valid for `'a`.
    Ok(unsafe { CStr::<Thin>::from_ptr(ptr::with_exposed_provenance(addr)) })
}

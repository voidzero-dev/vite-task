//! The backing-memory provider abstraction.
//!
//! The pool is generic over [`Sys`] so tests (and Miri) can substitute a
//! provider backed by the host allocator, while the real allocator obtains
//! anonymous mappings from the kernel (see the `mmap` module).

use core::ptr::NonNull;

/// Provides zero-initialized, aligned memory regions.
///
/// Implementations must themselves be async-signal-safe and fork-safe: no
/// locks, no thread-locals, no libc malloc.
pub trait Sys {
    /// Maps `size` bytes of zero-initialized memory aligned to `align`
    /// (a power of two). Returns `None` when memory is exhausted.
    fn map(size: usize, align: usize) -> Option<NonNull<u8>>;

    /// Releases a region previously returned by [`Sys::map`].
    ///
    /// # Safety
    ///
    /// `ptr` must have been returned by `Sys::map(size, align)` with the same
    /// `size` and `align`, and must not be accessed afterwards.
    unsafe fn unmap(ptr: NonNull<u8>, size: usize, align: usize);
}

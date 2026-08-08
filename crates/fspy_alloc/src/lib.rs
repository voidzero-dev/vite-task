//! Lock-free, async-signal-safe global allocator for the fspy preload library.
//!
//! The preload library interposes libc functions that POSIX declares
//! async-signal-safe (`open`, `stat`, `execve`, ...). Programs may call these
//! from signal handlers, and — more commonly — from the child of `fork()` in a
//! multithreaded process, where only async-signal-safe calls are permitted:
//! the libc allocator's locks may be held forever by threads that no longer
//! exist after the fork. Routing the preload's Rust allocations through this
//! allocator keeps them safe in both contexts:
//!
//! - **No locks.** Every state transition is a lock-free compare-and-swap
//!   loop: an attempt only retries because another running thread completed
//!   its operation, so nothing ever waits on state that a thread which
//!   vanished at `fork()` — or sits suspended under a signal handler — would
//!   have to release. (Lock-free, not wait-free: an individual operation has
//!   no fixed retry bound under active contention.)
//! - **No thread-locals.** TLS first-touch allocates through libc malloc on
//!   some platforms (macOS thread-local variables), which would reintroduce
//!   the hazard this crate exists to remove.
//! - **mmap-backed.** Memory comes straight from the kernel. On Linux the
//!   allocator relies on nothing from libc: mapping syscalls are issued
//!   directly (rustix's raw backend) and even the page size is discovered by
//!   probing with raw syscalls. On macOS, which has no stable raw-syscall
//!   ABI, calls go through the thin libSystem stubs. libc malloc is never
//!   called anywhere.
//!
//! Design: power-of-two size classes (16 B ..= 64 KiB) carve blocks out of
//! 1 MiB slabs; freed blocks recycle through a per-class Treiber free list
//! made ABA-safe by a generation tag. Requests larger than the biggest class
//! (or over-aligned beyond 4 KiB) map and unmap directly. See the `pool`
//! module for the details.
//!
//! Because the allocator is a `const`-initialized static with no lazy setup,
//! it works from the very first allocation in the process — even before the
//! preload library's constructor runs.

#![cfg_attr(not(test), no_std)]

// Compile as an empty crate on non-unix targets: the allocator backs the unix
// preload library. A Windows backend can be added alongside `sys::Mmap` if
// the Windows preload ever needs one.

#[cfg(unix)]
mod class;
#[cfg(unix)]
mod mapping;
#[cfg(unix)]
mod mmap;
#[cfg(unix)]
mod pool;
#[cfg(unix)]
mod slab;
#[cfg(unix)]
mod sys;

#[cfg(unix)]
use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::{self, NonNull},
};

#[cfg(unix)]
use crate::{mmap::Mmap, pool::Pool};

/// A lock-free, async-signal-safe, fork-safe [`GlobalAlloc`] implementation.
///
/// Intended to be installed as the `#[global_allocator]` of the fspy preload
/// library. All memory comes from anonymous mappings; libc malloc is never
/// called, no locks are taken, and no thread-local state is used.
///
/// Capacity is bounded by design: each size class can hold at most 256 slabs
/// of 1 MiB (roughly 200 MiB per class). Requests beyond that — far outside
/// anything the preload library does — fail like any other out-of-memory
/// condition (`alloc` returns null).
#[cfg(unix)]
pub struct FspyAlloc {
    pool: Pool<Mmap>,
}

#[cfg(unix)]
impl FspyAlloc {
    /// Creates the allocator. `const` so it can back a `static` with no
    /// runtime initialization.
    #[must_use]
    pub const fn new() -> Self {
        Self { pool: Pool::new() }
    }
}

#[cfg(unix)]
impl Default for FspyAlloc {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: `Pool` hands out blocks that are non-null, at least `layout.size()`
// bytes large, aligned to at least `layout.align()`, and exclusively owned
// until returned via `dealloc`. Allocation failure is reported as null, and
// none of the methods unwind.
#[cfg(unix)]
unsafe impl GlobalAlloc for FspyAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.pool.alloc(layout).map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let Some(ptr) = NonNull::new(ptr) else { return };
        // SAFETY: per the GlobalAlloc contract, `ptr` was returned by this
        // allocator for this `layout`.
        unsafe { self.pool.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.pool.alloc_zeroed(layout).map_or(ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some(ptr) = NonNull::new(ptr) else { return ptr::null_mut() };
        // SAFETY: per the GlobalAlloc contract, `ptr` was returned by this
        // allocator for this `layout`, and `new_size` is non-zero.
        unsafe { self.pool.realloc(ptr, layout, new_size) }.map_or(ptr::null_mut(), NonNull::as_ptr)
    }
}

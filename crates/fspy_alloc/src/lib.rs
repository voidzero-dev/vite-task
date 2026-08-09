//! Async-signal-safe allocation for the fspy preload library.
//!
//! The preload library interposes libc functions that POSIX declares
//! async-signal-safe (`open`, `stat`, `execve`, ...). Its code therefore runs
//! inside signal handlers and in the child of `fork()` in a multithreaded
//! process — contexts where taking libc malloc's locks can deadlock on a
//! suspended or vanished lock holder, so the preload's own allocations must
//! never go through libc malloc.
//!
//! This crate stacks three layers, and exposes only the top one, [`arena`].
//! `MmapAllocator` is the bottom: a stateless allocator where every
//! allocation is a fresh anonymous mapping from the kernel. `ChunkPool`
//! sits on top of it and caches fixed-size chunks, so that frequent short
//! tracing calls can reuse memory instead of paying two syscalls per call.
//! [`arena`] creates one `bump_scope::Bump` per intercepted call, drawing
//! its chunks from the process-wide pool and returning them on drop.

// Compile as an empty crate on non-unix targets: the allocator backs the
// unix preload library.
#![cfg(unix)]
#![cfg_attr(not(test), no_std)]

mod mmap;
mod pool;

use allocator_api2::alloc::Allocator;
use bump_scope::{
    Bump,
    alloc::compat::AllocatorApi2V02Compat,
    settings::{BumpAllocatorSettings, BumpSettings},
};
use mmap::MmapAllocator;
use pool::ChunkPool;

/// Every cached chunk is 64 KiB: a whole multiple of the page size on all
/// supported targets, and big enough that most intercepted calls fit their
/// allocations into a single chunk.
const CHUNK_SIZE: usize = 64 * 1024;
/// The alignment chunks are allocated with. Must be at least the alignment
/// `bump_scope::Bump` uses for its chunk requests — 16 (see
/// [`MmapAllocator`]'s docs for the links). bump-scope does not export that
/// constant, so the `bump_chunk_requests_fit_the_pool_gates` test pins the
/// fit instead: it fails if a bump-scope upgrade ever requests chunks the
/// pool would refuse.
const CHUNK_ALIGN: usize = 16;
/// At most this many chunks stay cached, capping retained memory at
/// `SLOTS * CHUNK_SIZE` = 4 MiB.
const SLOTS: usize = 64;

/// The process-wide chunk pool. `const`-initialized, so it works from the
/// first allocation on — even before any constructor has run.
static CHUNK_POOL: ChunkPool<MmapAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS> = ChunkPool::new();

/// `Bump::unallocated` requires its base allocator to implement `Default`
/// (an arena without chunks has nowhere to store an allocator value, so it
/// conjures one on first use). Point defaulted references at the
/// process-wide pool. As an allocator, `&ChunkPool` already works through
/// allocator-api2's blanket `impl Allocator for &A`.
impl Default for &'static ChunkPool<MmapAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS> {
    fn default() -> Self {
        &CHUNK_POOL
    }
}

/// Creates a fresh bump arena for one intercepted call, backed by the
/// process-wide chunk pool.
///
/// Creating the arena allocates nothing; the first allocation grabs a whole
/// chunk — usually a recycled one, so most calls touch no syscalls at all.
/// Deallocation only takes back the most recent allocation (bump-arena
/// semantics); everything is freed at once when the arena is dropped, and
/// its chunks go back to the pool.
///
/// The arena itself is single-owner — use one per call, do not share it
/// across threads. Creating one is safe anywhere, any time: the pool
/// underneath works in signal handlers and in the child of `fork()` (see
/// `ChunkPool` and `MmapAllocator` in this crate's source for why).
#[must_use]
pub fn arena() -> impl Allocator {
    // The default `Bump` settings, except the arena starts life without a
    // chunk, so creating one allocates nothing.
    type Settings = <BumpSettings as BumpAllocatorSettings>::WithGuaranteedAllocated<false>;
    Bump::<
        AllocatorApi2V02Compat<&'static ChunkPool<MmapAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>>,
        Settings,
    >::unallocated()
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use allocator_api2::alloc::Global;

    use super::*;

    /// Pins the fit between `Bump`'s chunk requests and the pool's gates
    /// (alignment at most [`CHUNK_ALIGN`], size at most [`CHUNK_SIZE`]).
    /// bump-scope keeps those request parameters private, so this test is
    /// the enforcement: it fails if an upgrade ever changes them.
    #[test]
    fn bump_chunk_requests_fit_the_pool_gates() {
        let pool = ChunkPool::<Global, CHUNK_SIZE, CHUNK_ALIGN, 4>::new_in(Global);
        // `try_new_in` requests the first chunk right away; if that request
        // were over-aligned or oversized, the pool would refuse and this
        // would be an error.
        let bump: Bump<AllocatorApi2V02Compat<&ChunkPool<Global, CHUNK_SIZE, CHUNK_ALIGN, 4>>> =
            Bump::try_new_in(AllocatorApi2V02Compat(&pool)).unwrap();
        let block = bump.allocate(Layout::from_size_align(100, 8).unwrap()).unwrap();
        let block_addr = block.cast::<u8>().as_ptr().addr();
        drop(bump);

        // The chunk went back into the pool's cache (proving it was served
        // as a chunk, not passed through): the next chunk-sized request
        // returns the block's surroundings.
        let recycled = pool.allocate(Layout::from_size_align(100, 8).unwrap()).unwrap();
        assert_eq!(recycled.len(), CHUNK_SIZE);
        let base = recycled.cast::<u8>().as_ptr().addr();
        assert!((base..base + CHUNK_SIZE).contains(&block_addr));
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(recycled.cast(), Layout::from_size_align(100, 8).unwrap()) };
    }

    #[test]
    #[cfg(not(miri))]
    fn arena_allocates_and_returns_chunks_to_the_pool() {
        let layout = Layout::from_size_align(100, 8).unwrap();

        let first_arena = arena();
        let first = first_arena.allocate(layout).unwrap();
        assert!(first.len() >= 100);
        // SAFETY: fresh exclusive block of at least 100 bytes.
        unsafe { first.cast::<u8>().as_ptr().write_bytes(0x5A, 100) };
        let second = first_arena.allocate(layout).unwrap();
        assert_ne!(first.cast::<u8>().as_ptr().addr(), second.cast::<u8>().as_ptr().addr());
        let first_addr = first.cast::<u8>().as_ptr().addr();
        // Everything dies at once; the chunk goes back to the pool.
        drop(first_arena);

        // No other test touches the process-wide pool, so a new arena draws
        // the same chunk back and its first allocation lands at the same
        // address.
        let second_arena = arena();
        let again = second_arena.allocate(layout).unwrap();
        assert_eq!(again.cast::<u8>().as_ptr().addr(), first_addr);
    }
}

//! Allocation that never touches libc malloc.
//!
//! Taking malloc's lock is the classic way for interposed code to deadlock a
//! traced program (see the crate docs), so the preload library allocates
//! through this crate instead. It stacks three layers: a stateless page
//! allocator at the bottom, where every allocation is fresh pages from
//! [`fspy_nostd::mm`] — an anonymous mapping on Unix, a `VirtualAlloc`
//! region on Windows; a process-wide `ChunkPool` above it that caches
//! fixed-size chunks; and `bump_scope::Bump`s on top. [`pooled_bump`]
//! creates a bump that draws its chunks from the pool and returns them on
//! drop, so frequent short-lived bumps reuse memory instead of paying two
//! syscalls each. [`page_bump`] bypasses the pool for bumps whose chunks
//! must never be recycled into other bumps.

#![cfg_attr(not(test), no_std)]

mod c_string;
#[cfg(unix)]
pub mod fs;
#[cfg(unix)]
mod mmap;
mod pool;
#[cfg(windows)]
mod virtual_alloc;

use allocator_api2::alloc::Allocator;
/// The bump interface [`pooled_bump`] returns, re-exported so callers can
/// name the bound and call its methods without a direct bump-scope
/// dependency.
pub use bump_scope::traits::BumpAllocator;
use bump_scope::{
    Bump,
    alloc::compat::AllocatorApi2V02Compat,
    settings::{BumpAllocatorSettings, BumpSettings},
};
pub use c_string::{CString, OsCString};
#[cfg(unix)]
pub use mmap::MmapAllocator as PageAllocator;
use pool::ChunkPool;
#[cfg(windows)]
pub use virtual_alloc::VirtualAllocator as PageAllocator;

/// Every cached chunk is 64 KiB: a whole multiple of the page size on all
/// supported targets, and big enough that most intercepted calls fit their
/// allocations into a single chunk. [`PooledBumpSettings`] pins the bumps'
/// own chunk sizing to this same value.
const CHUNK_SIZE: usize = 64 * 1024;
/// The alignment chunks are allocated with. Must be at least the alignment
/// `bump_scope::Bump` uses for its chunk requests — 16 (see
/// `MmapAllocator`'s docs for the links). bump-scope does not export that
/// constant, so the `bump_chunk_requests_fit_the_pool_gates` test pins the
/// fit instead: it fails if a bump-scope upgrade ever requests chunks the
/// pool would refuse.
const CHUNK_ALIGN: usize = 16;
/// At most this many chunks stay cached, capping retained memory at
/// `SLOTS * CHUNK_SIZE` = 4 MiB.
const SLOTS: usize = 64;

/// The process-wide chunk pool. `const`-initialized, so it works from the
/// first allocation on — even before any constructor has run.
static CHUNK_POOL: ChunkPool<PageAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS> = ChunkPool::new();

/// The `Bump` settings [`pooled_bump`] uses — the defaults, with two
/// changes:
///
/// - `WithGuaranteedAllocated<false>`: a bump starts life without a chunk,
///   so creating one allocates nothing.
/// - `WithMinimumChunkSize<CHUNK_SIZE>`: the bump's first chunk request is
///   sized to the pool's chunks, making the coupling explicit — rather than
///   relying on the pool rounding the default 512-byte first request up to a
///   whole chunk. (Both end up serving the same memory: the pool answers any
///   request up to `CHUNK_SIZE` with a whole chunk, and `Bump` uses the full
///   returned length.) The request comes out slightly *under* `CHUNK_SIZE` —
///   bump-scope deducts an assumed allocator-header overhead from its
///   minimum — which is exactly what keeps a minimum-sized request within
///   the pool's `size <= CHUNK_SIZE` gate; the
///   `bump_chunk_requests_fit_the_pool_gates` test pins that fit.
type PooledBumpSettings = <<BumpSettings as BumpAllocatorSettings>::WithGuaranteedAllocated<false> as BumpAllocatorSettings>::WithMinimumChunkSize<CHUNK_SIZE>;

/// `Bump::unallocated` requires its base allocator to implement `Default`
/// (a bump without chunks has nowhere to store an allocator value, so it
/// conjures one on first use). Point defaulted references at the
/// process-wide pool. As an allocator, `&ChunkPool` already works through
/// allocator-api2's blanket `impl Allocator for &A`.
impl Default for &'static ChunkPool<PageAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS> {
    fn default() -> Self {
        &CHUNK_POOL
    }
}

/// A bump allocator drawing whole pages straight from the kernel — never
/// from the chunk pool — so its memory is never recycled into other bumps.
///
/// The concrete type is public so a caller can house one in static
/// storage; note that a bump is not [`Sync`], so a `static` needs a cell
/// that hands out access, and no safe code can retain a leaked handle
/// globally.
pub type PageBump = Bump<AllocatorApi2V02Compat<PageAllocator>, PageBumpSettings>;

/// [`PageBump`]'s settings: start without a chunk, so creating one
/// allocates nothing.
pub type PageBumpSettings = <BumpSettings as BumpAllocatorSettings>::WithGuaranteedAllocated<false>;

/// Creates an empty [`PageBump`].
///
/// Creating it allocates nothing; the first allocation maps one chunk, and
/// further chunks are mapped only if the data outgrows it. Dropping the
/// bump frees its chunks; leaking it instead makes its allocations
/// permanent.
#[must_use]
pub const fn page_bump() -> PageBump {
    Bump::unallocated()
}

/// Creates a fresh bump backed by the process-wide chunk pool.
///
/// Creating the bump allocates nothing; the first allocation grabs a whole
/// chunk — usually a recycled one, so most uses touch no syscalls at all.
/// Deallocation only takes back the most recent allocation (bump
/// semantics); everything is freed at once when the bump is dropped, and
/// its chunks go back to the pool.
///
/// The bump is single-owner — do not share it across threads. Creating one
/// is safe anywhere, any time: the pool underneath works in signal
/// handlers, in the child of `fork()`, and under the Windows loader lock
/// (see `ChunkPool` and the platform page allocators in this crate's source
/// for why).
#[must_use]
pub fn pooled_bump() -> impl BumpAllocator + Allocator {
    Bump::<
        AllocatorApi2V02Compat<&'static ChunkPool<PageAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>>,
        PooledBumpSettings,
    >::unallocated()
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use allocator_api2::alloc::Global;

    use super::*;

    /// Pins the fit between `Bump`'s chunk requests and the pool's gates
    /// (alignment at most [`CHUNK_ALIGN`], size at most [`CHUNK_SIZE`]),
    /// under the same [`PooledBumpSettings`] the bumps use — including that
    /// a request under `WithMinimumChunkSize<CHUNK_SIZE>` still fits the
    /// `size <= CHUNK_SIZE` gate. bump-scope keeps its request parameters
    /// private, so this test is the enforcement: it fails if an upgrade
    /// ever changes them.
    /// [`PooledBumpSettings`] with `GuaranteedAllocated` flipped back on:
    /// without it, `Bump` demands a `Default` base allocator, and a
    /// reference to the test's stack-local pool cannot provide one. Chunk
    /// request sizing — what the test pins — is unaffected by that flag.
    type TestSettings =
        <PooledBumpSettings as BumpAllocatorSettings>::WithGuaranteedAllocated<true>;

    #[test]
    fn bump_chunk_requests_fit_the_pool_gates() {
        let pool = ChunkPool::<Global, CHUNK_SIZE, CHUNK_ALIGN, 4>::new_in(Global);
        // `try_new_in` requests the first chunk right away, at the settings'
        // minimum chunk size; if that request were over-aligned or
        // oversized, the pool would refuse and this would be an error.
        let bump: Bump<
            AllocatorApi2V02Compat<&ChunkPool<Global, CHUNK_SIZE, CHUNK_ALIGN, 4>>,
            TestSettings,
        > = Bump::try_new_in(AllocatorApi2V02Compat(&pool)).unwrap();
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
    fn pooled_bump_allocates_and_returns_chunks_to_the_pool() {
        let layout = Layout::from_size_align(100, 8).unwrap();

        let first_bump = pooled_bump();
        let first = first_bump.allocate(layout).unwrap();
        assert!(first.len() >= 100);
        // SAFETY: fresh exclusive block of at least 100 bytes.
        unsafe { first.cast::<u8>().as_ptr().write_bytes(0x5A, 100) };
        let second = first_bump.allocate(layout).unwrap();
        assert_ne!(first.cast::<u8>().as_ptr().addr(), second.cast::<u8>().as_ptr().addr());
        let first_addr = first.cast::<u8>().as_ptr().addr();
        // Everything dies at once; the chunk goes back to the pool.
        drop(first_bump);

        // No other test touches the process-wide pool, so a new bump draws
        // the same chunk back and its first allocation lands at the same
        // address.
        let second_bump = pooled_bump();
        let again = second_bump.allocate(layout).unwrap();
        assert_eq!(again.cast::<u8>().as_ptr().addr(), first_addr);
    }
}

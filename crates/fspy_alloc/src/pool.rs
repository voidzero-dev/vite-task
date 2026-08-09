//! A small cache of memory chunks between the bump arenas and the kernel.

use core::{
    alloc::Layout,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, Ordering},
};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::MmapAllocator;

/// A fixed-size cache of memory chunks.
///
/// Sits between the bump arenas and the underlying allocator `A` —
/// [`MmapAllocator`] in real use, any allocator in tests — so the chunks
/// that a finished arena gives back reach the next arena without going back
/// to the kernel.
///
/// The parameters are chosen by the layer above (the arena layer): every
/// cached chunk is exactly `CHUNK_SIZE` bytes and allocated with
/// `CHUNK_ALIGN`, which is also the strictest alignment the pool accepts
/// (`bump_scope::Bump` requests 16 — see [`MmapAllocator`]'s docs for the
/// links). The cache holds at most `SLOTS` chunks, so at most
/// `SLOTS * CHUNK_SIZE` bytes stay retained.
///
/// Requests of up to `CHUNK_SIZE` bytes are served with a whole chunk —
/// a cached one, or a fresh one when the cache is empty. Bigger requests go
/// straight to the underlying allocator. Deallocation mirrors that: chunks
/// go back into the cache (or to the underlying allocator when it is full),
/// bigger blocks go to the underlying allocator directly. The size passed
/// to `deallocate` tells the two apart exactly: for a block served as a
/// chunk, every size the caller may legally pass is at most `CHUNK_SIZE`;
/// for a pass-through block, every legal size is bigger.
///
/// Zero-sized requests and alignments above `CHUNK_ALIGN` are refused —
/// its only intended caller, `bump_scope::Bump`, never sends either.
///
/// # Why it is safe in signal handlers and forked children
///
/// The only state of the pool itself is a fixed array of atomic pointers,
/// one slot per cached chunk. Taking a chunk swaps a slot to null;
/// returning one swaps a null slot back. Every operation visits each slot
/// at most once and never waits — there are no locks to leave locked and no
/// lists to leave half-linked, so a thread that disappears mid-operation
/// (`fork`, a signal) can strand at most the one chunk it was holding,
/// never the pool. (The underlying allocator must give the same guarantee;
/// [`MmapAllocator`] does.)
///
/// The pool is `const`-constructible, so it can live in a `static` and
/// work before anything else has run.
pub struct ChunkPool<
    A: Allocator,
    const CHUNK_SIZE: usize,
    const CHUNK_ALIGN: usize,
    const SLOTS: usize,
> {
    slots: [AtomicPtr<u8>; SLOTS],
    allocator: A,
}

impl<const CHUNK_SIZE: usize, const CHUNK_ALIGN: usize, const SLOTS: usize>
    ChunkPool<MmapAllocator, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>
{
    #[must_use]
    pub const fn new() -> Self {
        Self::new_in(MmapAllocator)
    }
}

impl<A: Allocator, const CHUNK_SIZE: usize, const CHUNK_ALIGN: usize, const SLOTS: usize>
    ChunkPool<A, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>
{
    pub const fn new_in(allocator: A) -> Self {
        const {
            assert!(CHUNK_SIZE > 0, "CHUNK_SIZE must not be zero");
            assert!(CHUNK_ALIGN.is_power_of_two(), "CHUNK_ALIGN must be a power of two");
        }
        Self { slots: [const { AtomicPtr::new(ptr::null_mut()) }; SLOTS], allocator }
    }

    fn chunk_layout() -> Result<Layout, AllocError> {
        Layout::from_size_align(CHUNK_SIZE, CHUNK_ALIGN).map_err(|_| AllocError)
    }
}

impl<A: Allocator + Default, const CHUNK_SIZE: usize, const CHUNK_ALIGN: usize, const SLOTS: usize>
    Default for ChunkPool<A, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>
{
    fn default() -> Self {
        Self::new_in(A::default())
    }
}

// SAFETY: returned blocks are non-null, aligned to at least the accepted
// `layout.align()` (chunks are allocated with CHUNK_ALIGN, and stricter
// requests are refused), and at least `layout.size()` bytes large (chunks
// are exactly CHUNK_SIZE bytes and only serve requests up to that size;
// everything else comes from the underlying allocator with its own
// guarantees). A chunk leaves its slot before it is handed out and returns
// only on deallocation, so every block has exactly one owner and stays
// valid until deallocated.
unsafe impl<A: Allocator, const CHUNK_SIZE: usize, const CHUNK_ALIGN: usize, const SLOTS: usize>
    Allocator for ChunkPool<A, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>
{
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if layout.size() == 0 || layout.align() > CHUNK_ALIGN {
            return Err(AllocError);
        }
        if layout.size() > CHUNK_SIZE {
            return self.allocator.allocate(layout);
        }
        for slot in &self.slots {
            if slot.load(Ordering::Relaxed).is_null() {
                continue;
            }
            // Acquire pairs with the Release in `deallocate`; the swap makes
            // this thread the chunk's only owner. A racing taker may empty
            // the slot between the load and the swap — then the swap returns
            // null and the scan moves on.
            let chunk = slot.swap(ptr::null_mut(), Ordering::Acquire);
            if let Some(chunk) = NonNull::new(chunk) {
                return Ok(NonNull::slice_from_raw_parts(chunk, CHUNK_SIZE));
            }
        }
        // Cache empty: get a fresh chunk.
        self.allocator.allocate(Self::chunk_layout()?)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        if layout.size() > CHUNK_SIZE {
            // A pass-through block: only requests bigger than CHUNK_SIZE get
            // one, and all their legal deallocation sizes are bigger too.
            // SAFETY: caller contract — the block came from `allocate`
            // (which forwarded to the underlying allocator) with a fitting
            // layout.
            return unsafe { self.allocator.deallocate(ptr, layout) };
        }
        // A chunk: put it back into an empty slot, or give it back to the
        // underlying allocator if the cache is full. Release pairs with the
        // Acquire in `allocate`. A failed exchange means another thread just
        // filled the slot; move on.
        for slot in &self.slots {
            if !slot.load(Ordering::Relaxed).is_null() {
                continue;
            }
            if slot
                .compare_exchange(
                    ptr::null_mut(),
                    ptr.as_ptr(),
                    Ordering::Release,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return;
            }
        }
        let Ok(chunk_layout) = Self::chunk_layout() else { return };
        // SAFETY: every block in this branch is a CHUNK_SIZE chunk that came
        // from the underlying allocator with `chunk_layout` (caller contract
        // plus the size routing above), and it no longer has an owner.
        unsafe { self.allocator.deallocate(ptr, chunk_layout) };
    }
}

impl<A: Allocator, const CHUNK_SIZE: usize, const CHUNK_ALIGN: usize, const SLOTS: usize> Drop
    for ChunkPool<A, CHUNK_SIZE, CHUNK_ALIGN, SLOTS>
{
    fn drop(&mut self) {
        // Statics never drop; this runs for pools created in tests.
        for slot in &mut self.slots {
            let chunk = core::mem::replace(slot.get_mut(), ptr::null_mut());
            if let Some(chunk) = NonNull::new(chunk) {
                let Ok(chunk_layout) = Self::chunk_layout() else { return };
                // SAFETY: cached chunks came from the underlying allocator
                // with `chunk_layout`, and `&mut self` means no owner exists.
                unsafe { self.allocator.deallocate(chunk, chunk_layout) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, vec::Vec};

    use allocator_api2::alloc::Global;

    use super::*;

    const CHUNK_SIZE: usize = 64 * 1024;
    const CHUNK_ALIGN: usize = 16;

    fn pool() -> ChunkPool<Global, CHUNK_SIZE, CHUNK_ALIGN, 64> {
        ChunkPool::new_in(Global)
    }

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size, 8).unwrap()
    }

    #[test]
    fn small_requests_get_whole_recycled_chunks() {
        let pool = pool();
        let first = pool.allocate(layout(100)).unwrap();
        assert_eq!(first.len(), CHUNK_SIZE);
        let first_addr = first.cast::<u8>().as_ptr().addr();
        // SAFETY: fresh exclusive block of at least 100 bytes.
        unsafe { first.cast::<u8>().as_ptr().write_bytes(0xAB, 100) };
        // SAFETY: allocated above; the full returned size is a legal size to
        // pass back.
        unsafe { pool.deallocate(first.cast(), layout(CHUNK_SIZE)) };

        // The chunk is cached now; the next small request must get it back.
        let second = pool.allocate(layout(5000)).unwrap();
        assert_eq!(second.cast::<u8>().as_ptr().addr(), first_addr);
        assert_eq!(second.len(), CHUNK_SIZE);
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(second.cast(), layout(5000)) };
    }

    #[test]
    fn deallocating_with_the_requested_size_also_recycles() {
        let pool = pool();
        let block = pool.allocate(layout(100)).unwrap();
        let addr = block.cast::<u8>().as_ptr().addr();
        // SAFETY: allocated above; the requested size is a legal size to
        // pass back, and must still be recognized as a chunk.
        unsafe { pool.deallocate(block.cast(), layout(100)) };
        let again = pool.allocate(layout(100)).unwrap();
        assert_eq!(again.cast::<u8>().as_ptr().addr(), addr);
        // SAFETY: allocated above with the same layout.
        unsafe { pool.deallocate(again.cast(), layout(100)) };
    }

    #[test]
    fn big_requests_bypass_the_cache() {
        let pool = pool();
        let big = pool.allocate(layout(CHUNK_SIZE + 1)).unwrap();
        assert!(big.len() > CHUNK_SIZE);
        // SAFETY: fresh exclusive block of at least CHUNK_SIZE + 1
        // bytes.
        unsafe { big.cast::<u8>().as_ptr().write_bytes(0x5A, CHUNK_SIZE + 1) };
        // SAFETY: allocated above with the same layout.
        unsafe { pool.deallocate(big.cast(), layout(CHUNK_SIZE + 1)) };

        // A small request afterwards gets a chunk, not the big block.
        let small = pool.allocate(layout(100)).unwrap();
        assert_eq!(small.len(), CHUNK_SIZE);
        // SAFETY: allocated above with the same layout.
        unsafe { pool.deallocate(small.cast(), layout(100)) };
    }

    #[test]
    fn refuses_zero_size_and_over_chunk_alignment() {
        let pool = pool();
        let zero = Layout::from_size_align(0, 16).unwrap();
        assert!(pool.allocate(zero).is_err());
        let over_aligned = Layout::from_size_align(64, CHUNK_ALIGN * 2).unwrap();
        assert!(pool.allocate(over_aligned).is_err());
    }

    #[test]
    fn allocate_zeroed_scrubs_recycled_chunks() {
        let pool = pool();
        let block = pool.allocate(layout(256)).unwrap();
        // SAFETY: fresh exclusive block of at least 256 bytes.
        unsafe { block.cast::<u8>().as_ptr().write_bytes(0xFF, 256) };
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(block.cast(), layout(256)) };

        // Recycled chunks are dirty, so the default `allocate_zeroed` (which
        // we deliberately do not override here) must scrub them.
        let zeroed = pool.allocate_zeroed(layout(256)).unwrap();
        for i in 0..256 {
            // SAFETY: fresh exclusive block of at least 256 bytes.
            assert_eq!(unsafe { zeroed.cast::<u8>().as_ptr().add(i).read() }, 0);
        }
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(zeroed.cast(), layout(256)) };
    }

    #[test]
    fn overflowing_the_cache_is_fine() {
        // A tiny 4-slot pool so the full-cache path is genuinely exercised.
        let pool = ChunkPool::<Global, CHUNK_SIZE, CHUNK_ALIGN, 4>::new_in(Global);
        for _ in 0..2 {
            let blocks: Vec<_> = (0..8).map(|_| pool.allocate(layout(1000)).unwrap()).collect();
            for block in blocks {
                // SAFETY: allocated above with a fitting layout. The first 4
                // chunks fill the cache; the rest go back to the underlying
                // allocator.
                unsafe { pool.deallocate(block.cast(), layout(1000)) };
            }
        }
    }

    #[test]
    fn custom_chunk_size_and_alignment_work() {
        let pool = ChunkPool::<Global, 1024, 64, 8>::new_in(Global);
        let block = pool.allocate(layout(10)).unwrap();
        assert_eq!(block.len(), 1024);
        assert_eq!(block.cast::<u8>().as_ptr().addr() % 64, 0);
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(block.cast(), layout(10)) };

        // Bigger than this pool's chunk size: passes through.
        let big = pool.allocate(layout(2000)).unwrap();
        assert!(big.len() >= 2000);
        // SAFETY: allocated above with the same layout.
        unsafe { pool.deallocate(big.cast(), layout(2000)) };
    }

    #[test]
    fn concurrent_use_smoke() {
        const OPS: usize = if cfg!(miri) { 40 } else { 500 };

        let pool = pool();
        thread::scope(|scope| {
            for t in 0..4_usize {
                let pool = &pool;
                scope.spawn(move || {
                    for i in 0..OPS {
                        let size = 1 + (i * 37 + t * 101) % (2 * CHUNK_SIZE);
                        let l = Layout::from_size_align(size, 8).unwrap();
                        let block = pool.allocate(l).unwrap();
                        assert!(block.len() >= size);
                        let marker = u8::try_from((t * 50 + i) % 251).unwrap();
                        // SAFETY: exclusive fresh block of at least `size`
                        // bytes, allocated above with layout `l`.
                        unsafe {
                            block.cast::<u8>().as_ptr().write(marker);
                            block.cast::<u8>().as_ptr().add(size - 1).write(marker);
                            assert_eq!(block.cast::<u8>().as_ptr().read(), marker);
                            assert_eq!(block.cast::<u8>().as_ptr().add(size - 1).read(), marker);
                            pool.deallocate(block.cast(), l);
                        }
                    }
                });
            }
        });
    }

    /// The one test against the real kernel-backed default; everything else
    /// runs against `Global` so Miri can check it.
    #[test]
    #[cfg(not(miri))]
    fn mmap_backed_pool_smoke() {
        let pool = ChunkPool::<MmapAllocator, CHUNK_SIZE, CHUNK_ALIGN, 64>::new();
        let block = pool.allocate(layout(100)).unwrap();
        assert_eq!(block.len(), CHUNK_SIZE);
        let addr = block.cast::<u8>().as_ptr().addr();
        // SAFETY: fresh exclusive block of at least 100 bytes.
        unsafe { block.cast::<u8>().as_ptr().write_bytes(0xCD, 100) };
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(block.cast(), layout(100)) };
        let again = pool.allocate(layout(200)).unwrap();
        assert_eq!(again.cast::<u8>().as_ptr().addr(), addr);
        // SAFETY: allocated above with a fitting layout.
        unsafe { pool.deallocate(again.cast(), layout(200)) };
    }
}

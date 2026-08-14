//! Page-granularity allocator backed by private virtual-memory regions.

use core::{alloc::Layout, ptr::NonNull};

use allocator_api2::alloc::{AllocError, Allocator};
use fspy_nostd::mm::{AllocationType, PageProtection, virtual_alloc, virtual_free};

/// The Windows page size on every supported architecture.
const PAGE_SIZE: usize = 4096;

/// A stateless allocator: every allocation is a fresh private region from
/// `VirtualAlloc` and every deallocation a `VirtualFree` release.
///
/// The Windows counterpart of the Unix `MmapAllocator`, with the same
/// guarantee for the same reason: it holds no state at all — no locks, no
/// free lists, no thread-locals — and its two calls are kernel calls that
/// never take the CRT heap lock, so it works under the loader lock and in
/// every other context where the process heap must not be touched.
///
/// # What it accepts
///
/// The request profile matches `MmapAllocator`: any non-zero size, and any
/// alignment up to the page size. Regions are reserved at allocation
/// granularity (64 KiB), so every served alignment holds. Zero-sized
/// layouts and over-page alignments are refused with [`AllocError`].
///
/// # Cost
///
/// Every allocation reserves a fresh region and commits whole pages, and
/// every deallocation releases the region. Like its Unix counterpart, this
/// allocator is meant to sit below a chunk pool and bump arenas, not to
/// serve small allocations directly.
#[derive(Clone, Copy, Debug, Default)]
pub struct VirtualAllocator;

// SAFETY: returned blocks are non-null, allocation-granularity-aligned (at
// least `layout.align()` for every served layout), at least `layout.size()`
// bytes large (the returned length reports the committed size), stay valid
// until deallocated, and distinct regions never overlap.
unsafe impl Allocator for VirtualAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Outside the served request profile (see the type docs): refuse
        // rather than carry an over-alignment or dangling-block code path.
        if layout.size() == 0 || layout.align() > PAGE_SIZE {
            return Err(AllocError);
        }
        // `checked_next_multiple_of` is total: `None` on overflow, which is
        // already impossible — `Layout` caps sizes at `isize::MAX`.
        let size = layout.size().checked_next_multiple_of(PAGE_SIZE).ok_or(AllocError)?;
        let address = virtual_alloc(
            size,
            AllocationType::RESERVE | AllocationType::COMMIT,
            PageProtection::ReadWrite,
        )
        .map_err(|_| AllocError)?;
        Ok(NonNull::slice_from_raw_parts(address.cast::<u8>(), size))
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Freshly committed pages already read as zero.
        self.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, _layout: Layout) {
        // Releasing frees the whole region from its base, so the layout is
        // not needed to size the call.
        // SAFETY: caller contract — `ptr` was returned by `allocate`, making
        // it an unreleased region base, and the block is no longer in use.
        let _ = unsafe { virtual_free(ptr.cast()) };
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_aligned_zeroed_and_page_rounded() {
        for (size, align) in [(1, 1), (100, 64), (4096, 4096), (5 << 20, 8)] {
            let layout = Layout::from_size_align(size, align).unwrap();
            let block = VirtualAllocator.allocate(layout).unwrap();
            assert!(block.len() >= size, "size {size}");
            assert_eq!(block.len() % PAGE_SIZE, 0);
            assert_eq!(block.cast::<u8>().as_ptr().addr() % align, 0, "align {align}");
            for i in 0..block.len() {
                // SAFETY: fresh exclusive block of `block.len()` bytes.
                assert_eq!(unsafe { block.cast::<u8>().as_ptr().add(i).read() }, 0);
            }
            // SAFETY: fresh exclusive block of at least `size` bytes.
            unsafe { block.cast::<u8>().as_ptr().write_bytes(0x5A, size) };
            // SAFETY: allocated above; the layout fits the block.
            unsafe { VirtualAllocator.deallocate(block.cast(), layout) };
        }
    }

    #[test]
    fn deallocate_accepts_any_fitting_size() {
        let requested = Layout::from_size_align(100, 8).unwrap();
        let block = VirtualAllocator.allocate(requested).unwrap();
        // Deallocate with the *returned* size instead of the requested one —
        // both are within the fit range the contract allows.
        let fitting = Layout::from_size_align(block.len(), 8).unwrap();
        // SAFETY: allocated above; `fitting` is within the block's fit range.
        unsafe { VirtualAllocator.deallocate(block.cast(), fitting) };
    }

    #[test]
    fn out_of_profile_requests_are_refused() {
        // Zero-sized and over-page-aligned layouts are outside the served
        // request profile and must fail cleanly, not misbehave.
        let zero = Layout::from_size_align(0, 16).unwrap();
        assert!(VirtualAllocator.allocate(zero).is_err());
        let over_aligned = Layout::from_size_align(64, 1 << 24).unwrap();
        assert!(VirtualAllocator.allocate(over_aligned).is_err());
    }
}

//! Page-granularity allocator backed by anonymous memory mappings.

use core::{
    alloc::Layout,
    ptr::{self, NonNull},
};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::{
    mm::{MapFlags, ProtFlags, mmap_anonymous, munmap},
    param::page_size,
};

/// A stateless allocator: every allocation is a fresh anonymous mapping and
/// every deallocation an `munmap`.
///
/// # Why it is safe in signal handlers and forked children
///
/// It holds no state at all — no locks, no free lists, no thread-locals;
/// the kernel does all the bookkeeping. A signal or a `fork()` can never
/// catch it holding a lock or a half-written structure, because there is
/// nothing to hold. The mapping calls and the page-size read come from
/// [`crate::mm`] and [`crate::param`], which carry the same guarantee (see
/// their docs).
///
/// # What it accepts
///
/// The only intended caller is `bump_scope::Bump`, and `Bump` only ever
/// asks its base allocator for chunks ([single call site][site]). Chunks
/// are never zero-sized (a chunk always contains its own header), and
/// their alignment is [`max(MIN_CHUNK_ALIGN, header alignment)`][chunk-align],
/// [where `MIN_CHUNK_ALIGN` is 16][mca] — so 16 bytes in practice.
///
/// This allocator accepts more than that, because mapped memory gives the
/// extra range away for free: any non-zero size, and any alignment up to
/// the page size (mappings are always page-aligned). It refuses only two
/// kinds of request, which would each need extra code and never happen:
/// zero-sized layouts (they would need fake dangling blocks) and alignment
/// above the page size (it would need mapping extra space and trimming the
/// misaligned edges). The [`Allocator`] contract allows refusing any
/// request; refused requests get [`AllocError`].
///
/// # Cost
///
/// Every allocation takes whole pages (4 KiB at least, 16 KiB on Apple
/// silicon) and one syscall, and so does every deallocation. This
/// allocator is meant to sit below a chunk pool and bump arenas, not to
/// serve small allocations directly.
///
/// `allocate` returns the whole page-rounded block, and `bump_scope` [uses
/// the full returned length][fit], so none of the page is wasted.
///
/// [mca]: https://docs.rs/bump-scope/2.3.3/src/bump_scope/chunk/size_config.rs.html#8
/// [chunk-align]: https://docs.rs/bump-scope/2.3.3/src/bump_scope/chunk/size_config.rs.html#56-58
/// [site]: https://docs.rs/bump-scope/2.3.3/src/bump_scope/raw_bump.rs.html#865
/// [fit]: https://docs.rs/bump-scope/2.3.3/src/bump_scope/raw_bump.rs.html#873-884
#[derive(Clone, Copy, Debug, Default)]
pub struct MmapAllocator;

// SAFETY: returned blocks are non-null, page-aligned (at least
// `layout.align()` for every served layout), at least `layout.size()` bytes
// large (the returned length reports the exact mapped size), stay valid
// until deallocated, and distinct allocations never overlap.
unsafe impl Allocator for MmapAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let page = page_size();
        // Outside the served request profile (see the type docs): refuse
        // rather than carry an over-alignment or dangling-block code path.
        if layout.size() == 0 || layout.align() > page {
            return Err(AllocError);
        }
        // `checked_next_multiple_of` is total: `None` on overflow (already
        // impossible — `Layout` caps sizes at `isize::MAX`) or a zero page
        // size, with no power-of-two assumption to uphold.
        let size = layout.size().checked_next_multiple_of(page).ok_or(AllocError)?;
        // SAFETY: a fresh anonymous private mapping at no particular
        // address has no memory-safety preconditions.
        let ptr = unsafe {
            mmap_anonymous(
                ptr::null_mut(),
                size,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
        }
        .map_err(|_| AllocError)?;
        // Mapping results are page-aligned, which covers every served
        // layout.
        NonNull::new(ptr.cast::<u8>())
            .map(|ptr| NonNull::slice_from_raw_parts(ptr, size))
            .ok_or(AllocError)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Fresh anonymous mappings are already zero-filled by the kernel.
        self.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // The contract lets callers pass any size that fits the block, i.e.
        // anything in `[requested, mapped]`; every such size rounds up to
        // the mapped length. Zero-sized blocks are never allocated, so no
        // dangling pointers can arrive here. Rounding cannot fail for a
        // layout that fits a block we mapped; leaking the region is the
        // safe response if it somehow did.
        let Some(size) = layout.size().checked_next_multiple_of(page_size()) else { return };
        // SAFETY: caller contract — `ptr` was returned by `allocate` with a
        // fitting layout and the block is no longer in use. Failure is
        // impossible for a region we own.
        let _ = unsafe { munmap(ptr.as_ptr().cast(), size) };
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_aligned_zeroed_and_page_rounded() {
        for (size, align) in [(1, 1), (100, 64), (4096, 4096), (5 << 20, 8)] {
            let layout = Layout::from_size_align(size, align).unwrap();
            let block = MmapAllocator.allocate(layout).unwrap();
            assert!(block.len() >= size, "size {size}");
            assert_eq!(block.len() % page_size(), 0);
            assert_eq!(block.cast::<u8>().as_ptr().addr() % align, 0, "align {align}");
            for i in 0..block.len() {
                // SAFETY: fresh exclusive block of `block.len()` bytes.
                assert_eq!(unsafe { block.cast::<u8>().as_ptr().add(i).read() }, 0);
            }
            // SAFETY: fresh exclusive block of at least `size` bytes.
            unsafe { block.cast::<u8>().as_ptr().write_bytes(0x5A, size) };
            // SAFETY: allocated above; the layout fits the block.
            unsafe { MmapAllocator.deallocate(block.cast(), layout) };
        }
    }

    #[test]
    fn deallocate_accepts_any_fitting_size() {
        let requested = Layout::from_size_align(100, 8).unwrap();
        let block = MmapAllocator.allocate(requested).unwrap();
        // Deallocate with the *returned* size instead of the requested one —
        // both are within the fit range the contract allows.
        let fitting = Layout::from_size_align(block.len(), 8).unwrap();
        // SAFETY: allocated above; `fitting` is within the block's fit range.
        unsafe { MmapAllocator.deallocate(block.cast(), fitting) };
    }

    #[test]
    fn out_of_profile_requests_are_refused() {
        // Zero-sized and over-page-aligned layouts are outside the served
        // request profile and must fail cleanly, not misbehave.
        let zero = Layout::from_size_align(0, 16).unwrap();
        assert!(MmapAllocator.allocate(zero).is_err());
        let over_aligned = Layout::from_size_align(64, 1 << 24).unwrap();
        assert!(MmapAllocator.allocate(over_aligned).is_err());
    }
}

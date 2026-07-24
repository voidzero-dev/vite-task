use std::{
    num::NonZeroUsize,
    ptr::NonNull,
    sync::atomic::{AtomicUsize, Ordering},
};

use nix::sys::mman::{MapFlags, MmapAdvise, ProtFlags, madvise, mmap_anonymous, munmap};

const INITIAL_GENERATION: usize = 1;
const MARKER_SIZE: NonZeroUsize = NonZeroUsize::new(size_of::<AtomicUsize>()).unwrap();

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ForkGeneration(usize);

pub struct ForkTracker {
    marker: NonNull<AtomicUsize>,
    next_generation: AtomicUsize,
}

impl ForkTracker {
    #[cfg_attr(
        test,
        expect(dead_code, reason = "the preload constructor is disabled in unit-test builds")
    )]
    pub fn new() -> nix::Result<Self> {
        // SAFETY: This creates a private anonymous mapping with a nonzero size.
        let marker = unsafe {
            mmap_anonymous(
                None,
                MARKER_SIZE,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_PRIVATE,
            )
        }?;

        if let Err(error) = unsafe {
            // SAFETY: `marker` identifies the mapping created above for its
            // entire length. MADV_WIPEONFORK leaves the mapping in children
            // but zeroes it.
            madvise(marker, MARKER_SIZE.get(), MmapAdvise::MADV_WIPEONFORK)
        } {
            // SAFETY: The mapping is still owned here and has its original size.
            let _ = unsafe { munmap(marker, MARKER_SIZE.get()) };
            return Err(error);
        }

        let marker = marker.cast::<AtomicUsize>();
        // SAFETY: The mapping is writable, aligned to a page boundary, and
        // large enough for one AtomicUsize.
        unsafe { marker.as_ptr().write(AtomicUsize::new(INITIAL_GENERATION)) };

        Ok(Self { marker, next_generation: AtomicUsize::new(INITIAL_GENERATION) })
    }

    pub fn generation(&self) -> ForkGeneration {
        let marker = self.marker();
        let current = marker.load(Ordering::Acquire);
        if current != 0 {
            return ForkGeneration(current);
        }

        loop {
            let candidate = self.next_generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
            if candidate == 0 {
                continue;
            }

            match marker.compare_exchange(0, candidate, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return ForkGeneration(candidate),
                Err(current) => {
                    debug_assert_ne!(current, 0);
                    return ForkGeneration(current);
                }
            }
        }
    }

    const fn marker(&self) -> &AtomicUsize {
        // SAFETY: `self.marker` remains mapped for the lifetime of `self`.
        unsafe { self.marker.as_ref() }
    }
}

impl Drop for ForkTracker {
    fn drop(&mut self) {
        // SAFETY: `self.marker` owns the mapping with this exact size.
        let _ = unsafe { munmap(self.marker.cast(), MARKER_SIZE.get()) };
    }
}

// SAFETY: The mapping contains one AtomicUsize and all access uses atomic
// operations. The mapping address remains stable for the tracker's lifetime.
unsafe impl Send for ForkTracker {}
// SAFETY: See the Send implementation; concurrent access is atomic.
unsafe impl Sync for ForkTracker {}

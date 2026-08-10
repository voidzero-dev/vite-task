//! A lock-free frame arena in a shared memory region.
//!
//! Region layout:
//!
//! ```text
//! | total byte size of frames: AtomicUsize | frame | frame | ... |
//! ```
//!
//! and every frame is
//!
//! ```text
//! | size: u32 | content | padding to the next 4-byte boundary |
//! ```
//!
//! One atomic word — the end offset — carries the whole arena. Writers claim
//! disjoint ranges out of it with a single `try_update`, then write their
//! header once with a plain store: the claim already made the range exclusive,
//! and what publishes it to the reader is the gate release in the parent module,
//! not the header store.
//!
//! There is exactly one frame state, so nothing here tolerates a crashed or
//! half-finished writer. It does not have to: [`super::GatedShmReceiver::close`]
//! hands out a reader only when every claim it admitted ran to completion, and a
//! writer that dies mid-frame leaks its gate count instead, so the run is never
//! read at all.

use core::iter::from_fn;
use std::{
    num::NonZeroUsize,
    ptr::slice_from_raw_parts_mut,
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};

use bytemuck::must_cast;
use fspy_shm::Mapping;

/// Type of a frame's size header.
type FrameSize = u32;

// The end offset is written with atomic operations and read back with a plain
// load, so the two layouts have to agree.
const _: () = {
    assert!(size_of::<usize>() == size_of::<AtomicUsize>());
    assert!(align_of::<usize>() == align_of::<AtomicUsize>());
};

/// A trait to borrow a raw memory region.
pub trait AsRawSlice {
    fn as_raw_slice(&self) -> *mut [u8];
}

impl AsRawSlice for Mapping {
    fn as_raw_slice(&self) -> *mut [u8] {
        slice_from_raw_parts_mut(self.as_ptr(), self.len())
    }
}

#[track_caller]
fn assert_alignment(ptr: *const u8) {
    // The arena's end offset is a `usize`.
    assert_eq!(ptr as usize % align_of::<usize>(), 0);
    // Every frame header that follows it is a `FrameSize`.
    assert_eq!((ptr as usize + size_of::<usize>()) % align_of::<FrameSize>(), 0);
}

const fn roundup_to_align_frame_header(size: usize) -> usize {
    size.next_multiple_of(align_of::<FrameSize>())
}

/// A concurrent frame arena writer.
///
/// Lock-free and safe to use from several threads and processes at once: the end
/// offset gives every claim a range of its own.
pub(super) struct ShmWriter<M> {
    mem: M,
}

impl<M: AsRawSlice> ShmWriter<M> {
    /// Creates a writer over a shared memory region.
    ///
    /// # Safety
    /// - `mem.as_raw_slice()` must return a stable valid pointer to the region,
    /// - the region must only be accessed via `ShmWriter` and `ShmReader` across
    ///   all the processes,
    /// - the region must be zero-initialized before first use.
    pub(super) unsafe fn new(mem: M) -> Self {
        assert_alignment(mem.as_raw_slice() as *const u8);
        Self { mem }
    }

    /// Claims `frame_size` bytes of frame content and returns them.
    ///
    /// Returns `None` when the arena has no room left, or when the frame is
    /// larger than a size header can describe.
    ///
    /// The header is written before returning. Publishing the content to a
    /// reader is the caller's job, and its gate guard does that on drop.
    #[expect(
        clippy::mut_from_ref,
        reason = "every claim carves out a range no other claim can reach, which is what makes \
                  the exclusive borrow sound"
    )]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "`new` asserts the base alignment and the claim keeps frame headers aligned"
    )]
    pub(super) fn claim_frame(&self, frame_size: NonZeroUsize) -> Option<&mut [u8]> {
        let shm_slice: *mut [u8] = self.mem.as_raw_slice();
        let shm_ptr = shm_slice.cast::<u8>();
        let shm_len = shm_slice.len();

        let frame_size = frame_size.get();
        // The header describes a frame's size in a `FrameSize`, so that is also
        // the largest frame this arena can hold.
        let Ok(frame_size_header) = FrameSize::try_from(frame_size) else {
            return None;
        };

        // The end position of all frames lives in the region's first machine word.
        // SAFETY: `shm_ptr` points to the start of the region, which is aligned to
        // `usize` (verified by `assert_alignment` in `new`) and large enough to
        // contain at least a `usize` header.
        let end_offset = unsafe { AtomicUsize::from_ptr(shm_ptr.cast()) };

        let frame_with_header_size = size_of::<FrameSize>() + frame_size;

        // Claim the space. Writers share only this word, never each other's
        // content, so relaxed ordering is enough.
        let current_end = end_offset
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current_end| {
                // Round up so that the next frame's header stays aligned.
                let new_end = roundup_to_align_frame_header(current_end + frame_with_header_size);
                if size_of::<usize>() + new_end > shm_len {
                    return None;
                }
                Some(new_end)
            })
            .ok()?;

        // SAFETY: the atomic claim above guaranteed that
        // `size_of::<usize>() + current_end` is within the region's bounds, so
        // this pointer arithmetic stays inside the allocation.
        let frame_start = unsafe { shm_ptr.add(size_of::<usize>() + current_end) };

        // SAFETY: `frame_start` is aligned for a `FrameSize` (ensured by
        // `roundup_to_align_frame_header`) and lies inside the range this writer
        // just claimed exclusively, so nothing else can race this store.
        unsafe { frame_start.cast::<FrameSize>().write(frame_size_header) };

        // SAFETY: skipping the header stays inside the claimed range.
        let content_ptr = unsafe { frame_start.add(size_of::<FrameSize>()) };
        // SAFETY: `content_ptr` is valid for `frame_size` bytes (guaranteed by the
        // atomic claim), aligned for `u8`, and no other writer will touch this
        // range because every writer claims a range of its own.
        Some(unsafe { slice::from_raw_parts_mut(content_ptr, frame_size) })
    }
}

/// Reader of the frames an [`ShmWriter`] appended.
///
/// `mem` must cover everything a writer produced and must no longer be written.
/// `M: AsRef<[u8]>` makes the local half of that borrow-checked; the
/// cross-process half is the caller's responsibility. Here it is discharged by
/// [`super::GatedShmReceiver::close`], which produces a reader only once the gate
/// proved that every claimed frame was fully written and published.
pub(super) struct ShmReader<M: AsRef<[u8]>> {
    mem: M,
}

impl<M: AsRef<[u8]>> ShmReader<M> {
    pub(super) fn new(mem: M) -> Self {
        assert_alignment(mem.as_ref().as_ptr());
        Self { mem }
    }

    /// Iterates over every frame in the arena, in claim order.
    pub(super) fn iter_frames(&self) -> impl Iterator<Item = &[u8]> {
        let mem = self.mem.as_ref();
        let (header, content) = mem
            .split_first_chunk::<{ size_of::<usize>() }>()
            .expect("mem too small to contain header");
        let content_size: usize = must_cast(*header);
        let mut remaining_content = &content[..content_size];

        from_fn(move || {
            let (frame_header, next_remaining_content) =
                remaining_content.split_first_chunk::<{ size_of::<FrameSize>() }>()?;
            let frame_size = usize::try_from(must_cast::<_, FrameSize>(*frame_header))
                .expect("frame size exceeds usize");
            // Claims are `NonZeroUsize`, so a zero header means the region does
            // not hold what this reader was promised. Say so rather than loop.
            assert!(frame_size != 0, "shared memory holds a frame of size zero");

            let (frame_with_padding, next_remaining_content) =
                next_remaining_content.split_at(roundup_to_align_frame_header(frame_size));
            remaining_content = next_remaining_content;

            Some(&frame_with_padding[..frame_size])
        })
    }
}

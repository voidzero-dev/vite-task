//! The writer side: claiming, filling, and committing frames.

use std::{
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
};

use super::{
    AsRawSlice, slot,
    state::{ReserveError, SharedState},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use across threads and processes at the same time: frames are
/// reserved with atomic operations, filled in uniquely owned payload spans,
/// and published with an atomic commit (see the module docs of
/// [`super::state`]).
pub struct ShmWriter<M> {
    mem: M,
}

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimError {
    /// The receiver closed the channel; anything after this point is
    /// outside the channel's boundary.
    #[error("the channel has been closed by the receiver")]
    Closed,
    /// The region is full. The claim's counter bumps already recorded the
    /// loss, so the channel will report itself incomplete.
    #[error("no space left in the shared-memory region")]
    Capacity,
}

impl<M: AsRawSlice> ShmWriter<M> {
    /// Creates a writer backed by a shared-memory region.
    ///
    /// # Safety
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the writer's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    ///
    /// # Panics
    ///
    /// Panics when the region is not `u64`-aligned or its size is outside the
    /// supported range (see [`SharedState::borrow`]).
    pub unsafe fn new(mem: M) -> Self {
        // Validate the region geometry eagerly so misuse fails at
        // construction, not at the first claim.
        // SAFETY: forwarded from this function's contract.
        let _ = unsafe { SharedState::borrow(mem.as_raw_slice()) };
        Self { mem }
    }

    fn state(&self) -> SharedState<'_> {
        // SAFETY: `new` requires the region to stay valid and
        // protocol-governed for the writer's lifetime, and it validated the
        // geometry.
        unsafe { SharedState::borrow(self.mem.as_raw_slice()) }
    }

    /// Whether the receiver has closed the channel.
    pub fn is_closed(&self) -> bool {
        self.state().is_closed()
    }

    /// Claims a frame of exactly `frame_size` bytes.
    ///
    /// The frame is invisible to the receiver until [`FrameMut::finish`]
    /// commits it. Dropping the frame without finishing abandons the claim:
    /// the receiver ignores the slot, exactly as if the writer had died.
    ///
    /// # Panics
    ///
    /// Panics when `frame_size` exceeds the frame limit of `i32::MAX`
    /// bytes.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let state = self.state();
        let reservation = state.try_claim(frame_size.get()).map_err(|err| match err {
            ReserveError::Closed => ClaimError::Closed,
            ReserveError::Capacity => ClaimError::Capacity,
        })?;

        let content_ptr = state.payload_ptr(reservation.payload_offset);
        // SAFETY: the allocator compare-and-swap reserved
        // `[payload_offset, payload_offset + frame_size)` exclusively for
        // this frame: other writers reserve disjoint spans, and the receiver
        // never reads a payload before observing its committed descriptor —
        // which `finish` publishes only when it consumes this borrow.
        let content = unsafe { std::slice::from_raw_parts_mut(content_ptr, frame_size.get()) };
        Ok(FrameMut {
            state,
            slot_index: reservation.slot_index,
            descriptor: slot::committed(reservation.payload_offset, frame_size.get()),
            content,
        })
    }

    // Unwrap `self` and return the underlying memory.
    #[cfg(test)]
    pub fn into_memory(self) -> M {
        self.mem
    }

    #[cfg(test)]
    pub fn try_write_frame(&self, frame: &[u8]) -> bool {
        let Some(frame_size) = NonZeroUsize::new(frame.len()) else {
            return false;
        };
        let Ok(mut frame_mut) = self.claim_frame(frame_size) else {
            return false;
        };
        frame_mut.copy_from_slice(frame);
        frame_mut.finish();
        true
    }
}

/// An exclusively owned, claimed-but-unpublished frame.
///
/// [`FrameMut::finish`] commits the frame; it is the only way to make the
/// payload visible to the receiver. Dropping the frame instead abandons
/// the claim: the slot stays unfinished and the receiver ignores it,
/// exactly as if the writer had died there. A writer that abandons a frame
/// and still performs the operation it described steps outside the usage
/// contract — records are published before the recorded operation.
pub struct FrameMut<'a> {
    state: SharedState<'a>,
    slot_index: usize,
    descriptor: u64,
    content: &'a mut [u8],
}

impl std::fmt::Debug for FrameMut<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameMut")
            .field("slot_index", &self.slot_index)
            .field("len", &self.content.len())
            .finish_non_exhaustive()
    }
}

impl Deref for FrameMut<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.content
    }
}

impl DerefMut for FrameMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.content
    }
}

impl FrameMut<'_> {
    /// Commits the frame, making it visible to the receiver.
    ///
    /// If the receiver closed the channel and aborted this frame's slot
    /// first, the frame is silently discarded: the record belongs to the
    /// close race and is intentionally excluded either way.
    pub fn finish(self) {
        self.state.commit(self.slot_index, self.descriptor);
    }
}

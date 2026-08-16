//! The writer side: claim a frame, fill it, finish it.

use std::{
    fmt,
    num::{NonZeroU32, NonZeroUsize},
    ops::{Deref, DerefMut},
    slice,
    sync::atomic::Ordering,
};

use super::{
    AsRawSlice,
    layout::{self, CLOSED, MappedLayout, SlotState},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use across threads and processes at the same time: frames are
/// reserved with atomic operations, filled in uniquely owned payload spans,
/// and published with an atomic commit (see the ordering contract above).
pub struct ShmWriter<M, const SLOTS: usize> {
    mapped: MappedLayout<SLOTS>,
    /// Owns the region the views point into. Declared after them: fields
    /// drop in order, and what borrows must die before what is borrowed.
    _mem: M,
}

// SAFETY: the writer touches the region only through the protocol's
// atomics, which synchronize access from any thread; the stored views
// point into the mapping's stable, independently owned target, not into
// the writer value itself.
unsafe impl<M: Send, const SLOTS: usize> Send for ShmWriter<M, SLOTS> {}
// SAFETY: see the `Send` impl; the writer's shared-reference API is
// internally synchronized by the protocol.
unsafe impl<M: Sync, const SLOTS: usize> Sync for ShmWriter<M, SLOTS> {}

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimError {
    /// The CLOSED gate was set: the receiver sealed the channel, or an
    /// earlier failed claim condemned it. Skipping the record is sound
    /// either way — it is outside the receiver's boundary, or the same
    /// bit already makes the receiver report the channel incomplete.
    #[error("the channel has been closed")]
    Closed,
    /// The claim was refused for space: the region was full, or the frame
    /// was larger than the `u32::MAX`-byte frame limit. The loss is
    /// already recorded — this claim set the CLOSED gate — so the channel
    /// will report itself incomplete and refuse further claims.
    #[error("no space left in the shared-memory region")]
    Capacity,
}

impl<M: AsRawSlice, const SLOTS: usize> ShmWriter<M, SLOTS> {
    /// Creates a writer backed by a shared-memory region, or `None` when
    /// the region cannot host the protocol (see [`MappedLayout::new`]) —
    /// a truncated or foreign file, for a sender that did not create it.
    ///
    /// # Safety
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the writer's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    pub unsafe fn new(mem: M) -> Option<Self> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid and protocol-governed for the writer's lifetime —
        // and so for every use of the views, which are stored in and
        // dropped with the writer.
        let mapped = unsafe { MappedLayout::new(mem.as_raw_slice()) }?;
        Some(Self { mapped, _mem: mem })
    }

    /// Whether the CLOSED gate is set: the receiver sealed the channel,
    /// or an earlier failed claim condemned it.
    pub fn is_closed(&self) -> bool {
        self.mapped.claims().load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Claims a frame of exactly `frame_size` bytes.
    ///
    /// The frame is invisible to the receiver until [`FrameMut::finish`]
    /// commits it. Dropping the frame without finishing abandons the claim:
    /// the receiver ignores the slot, exactly as if the writer had died.
    /// Frames larger than `u32::MAX` bytes are refused as
    /// [`ClaimError::Capacity`].
    ///
    /// Wait-free: two `fetch_add`s, no retry loop (rule 1). A claim that
    /// does not fit fails after setting the CLOSED gate: the receiver
    /// learns a record was lost, and later claims are refused — their
    /// records would ride a result the receiver must already reject.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_, SLOTS>, ClaimError> {
        let mapped = self.mapped;
        let payload_len = frame_size.get();

        // Reports that this claim's record was lost, before the writer
        // moves on (rule 1): the gate makes the receiver report the
        // channel incomplete, and condemns further claims.
        let report_loss = || {
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);
            ClaimError::Capacity
        };

        // The descriptor's 32-bit nonzero length field is the oversize
        // check: refuse, before touching the counters, a frame it cannot
        // describe — the channel keeps working for every record after it.
        let Ok(encoded_len) = NonZeroU32::try_from(frame_size) else {
            return Err(report_loss());
        };
        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot.
        let payload_start =
            mapped.payload_reserved().fetch_add(payload_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(payload_len as u64);
        if payload_end.is_none_or(|end| end > mapped.payloads.len() as u64) {
            let err = report_loss();
            // Net-zero refusal, gate first (rule 1): undo this claim's own
            // reservation. It lay beyond the region, so no live span sits
            // above it and the counter cannot dip below one.
            mapped.payload_reserved().fetch_sub(payload_len as u64, Ordering::Relaxed);
            return Err(err);
        }
        // Bounded by the capacity check: the payload region fits 32-bit
        // offsets.
        let payload_offset = u32::try_from(payload_start).expect("bounded by the payload region");
        let payload_start = payload_offset as usize;

        let claims = mapped.claims().fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            // Not a loss: a record refused after the seal describes an
            // operation performed outside the channel's boundary. Net-zero
            // refusal (rule 1): the gate was observed set, so undo the
            // increment — the counter holds still instead of drifting
            // toward the gate bit. (The payload reservation stays: in
            // bounds, never materialized, and capped by the region.)
            mapped.claims().fetch_sub(1, Ordering::Relaxed);
            return Err(ClaimError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= SLOTS {
            let err = report_loss();
            // Net-zero refusal, gate first (rule 1): once the gate is in
            // the word, every later read of the counter carries it, so no
            // claim can mistake the undone value for an open channel.
            mapped.claims().fetch_sub(1, Ordering::Relaxed);
            return Err(err);
        }

        // SAFETY: the claim reserved
        // `[payload_start, payload_start + payload_len)` — inside the
        // payload region by the capacity check above — exclusively for this
        // frame: other writers reserve disjoint (if byte-adjacent) spans,
        // and the receiver never reads a payload before observing its
        // committed descriptor, which `finish` publishes only when it
        // consumes this borrow.
        let content = unsafe {
            slice::from_raw_parts_mut(mapped.payloads.cast::<u8>().add(payload_start), payload_len)
        };
        Ok(FrameMut {
            mapped,
            slot_index,
            descriptor: SlotState::Committed { offset: payload_offset, len: encoded_len }.encode(),
            content,
        })
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
pub struct FrameMut<'a, const SLOTS: usize> {
    mapped: MappedLayout<SLOTS>,
    slot_index: usize,
    descriptor: u64,
    content: &'a mut [u8],
}

impl<const SLOTS: usize> fmt::Debug for FrameMut<'_, SLOTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameMut")
            .field("slot_index", &self.slot_index)
            .field("len", &self.content.len())
            .finish_non_exhaustive()
    }
}

impl<const SLOTS: usize> Deref for FrameMut<'_, SLOTS> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.content
    }
}

impl<const SLOTS: usize> DerefMut for FrameMut<'_, SLOTS> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.content
    }
}

impl<const SLOTS: usize> FrameMut<'_, SLOTS> {
    /// Commits the frame, making it visible to the receiver.
    ///
    /// If the receiver sealed the channel and froze this frame's slot
    /// first, the swap fails and the frame is silently discarded: the
    /// record belongs to the seal race and is intentionally excluded
    /// either way.
    pub fn finish(self) {
        // Rule 2: `Release` orders every payload write before the
        // descriptor.
        let _ = self.mapped.table()[self.slot_index].compare_exchange(
            layout::UNFINISHED,
            self.descriptor,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

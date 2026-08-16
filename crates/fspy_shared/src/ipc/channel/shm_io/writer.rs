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
    layout::{self, CLOSED, MappedLayout, SlotState, to_usize},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use from many threads and processes at once: each frame is
/// reserved atomically, filled in a span no one else can touch, and
/// published with one atomic write (the ordering contract in [`layout`]).
pub struct ShmWriter<M, const SLOTS: usize> {
    mapped: MappedLayout<SLOTS>,
    /// Owns the region the pointers point into. Declared after them:
    /// fields drop in order, and the borrower must go first.
    _mem: M,
}

// SAFETY: the writer touches the region only through the protocol's
// atomics, which synchronize access from any thread; the stored pointers
// point into the mapped memory, which is owned separately and does not
// move, not into the writer itself.
unsafe impl<M: Send, const SLOTS: usize> Send for ShmWriter<M, SLOTS> {}
// SAFETY: see the `Send` impl; the writer's shared-reference API is
// internally synchronized by the protocol.
unsafe impl<M: Sync, const SLOTS: usize> Sync for ShmWriter<M, SLOTS> {}

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimError {
    /// The CLOSED gate was set: the receiver sealed the channel, or an
    /// earlier claim failed and shut it down. Dropping the record is
    /// right either way — the receiver had already stopped collecting,
    /// or that same bit already tells it the frames are incomplete.
    #[error("the channel has been closed")]
    Closed,
    /// There was no room: the region is full, or the frame is longer than
    /// the `u32::MAX`-byte limit. The loss is already recorded — this
    /// claim set the CLOSED gate — so the channel now reports itself
    /// incomplete and refuses every later claim.
    #[error("no space left in the shared-memory region")]
    Capacity,
}

impl<M: AsRawSlice, const SLOTS: usize> ShmWriter<M, SLOTS> {
    /// Creates a writer on a shared-memory region, or `None` when the
    /// region cannot hold the protocol (see [`MappedLayout::new`]) — a
    /// truncated or unrelated file, for a sender that did not create it.
    ///
    /// # Safety
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the writer's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    pub unsafe fn new(mem: M) -> Option<Self> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid, and used only by this protocol, for as long as the
        // writer lives — and so for every use of the pointers, which are
        // stored in the writer and dropped with it.
        let mapped = unsafe { MappedLayout::new(mem.as_raw_slice()) }?;
        Some(Self { mapped, _mem: mem })
    }

    /// Whether the CLOSED gate is set: the receiver sealed the channel,
    /// or an earlier claim failed and shut it down.
    pub fn is_closed(&self) -> bool {
        self.mapped.claims().load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Claims a frame of exactly `frame_size` bytes. Wait-free: two
    /// `fetch_add`s, no retry loop (rule 1).
    ///
    /// The receiver cannot see the frame until [`FrameMut::finish`]
    /// commits it; dropping it instead gives the claim up, and the
    /// receiver ignores the slot exactly as if the writer had died. A
    /// claim that does not fit — the region is full, or `frame_size` is
    /// over `u32::MAX` — fails as [`ClaimError::Capacity`] after setting
    /// the CLOSED gate.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_, SLOTS>, ClaimError> {
        let mapped = self.mapped;

        // The loss report (rule 1): the gate marks the frames incomplete
        // and shuts the channel down for later claims.
        let report_loss = || {
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);
            ClaimError::Capacity
        };

        // A frame too long for the descriptor's 32-bit length field cannot
        // be described, so this conversion is the oversize check. No
        // counter has moved yet, so there is nothing to undo.
        let Ok(frame_size) = NonZeroU32::try_from(frame_size) else {
            return Err(report_loss());
        };
        // The payload space this claim takes, as the counter's `u64`.
        // Widening a 32-bit size never loses anything.
        let reservation = u64::from(frame_size.get());
        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot.
        let payload_offset = mapped.payload_reserved().fetch_add(reservation, Ordering::Relaxed);
        // Checked: if something else scribbled on the counter, the claim
        // must fail rather than wrap around into a span outside the
        // region.
        let payload_end = payload_offset.checked_add(reservation);
        if payload_end.is_none_or(|end| end > u64::from(mapped.payload_len)) {
            let err = report_loss();
            // Put it back (rule 1), gate first. The reservation ran off
            // the end of the region, so no live frame sits above it.
            mapped.payload_reserved().fetch_sub(reservation, Ordering::Relaxed);
            return Err(err);
        }
        // In range by the check above, and the payload area is short
        // enough for 32-bit offsets.
        let payload_offset = u32::try_from(payload_offset).expect("bounded by the payload region");

        let claims = mapped.claims().fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            // Not a loss: the receiver had already stopped collecting
            // when this record was refused. Put the count back (rule 1)
            // — the gate was already set. (The payload reservation
            // stays: it is inside the region, nothing was written
            // there, and the region caps how much can pile up.)
            mapped.claims().fetch_sub(1, Ordering::Relaxed);
            return Err(ClaimError::Closed);
        }
        let slot_index = to_usize(claims);
        if slot_index >= SLOTS {
            let err = report_loss();
            // Put the count back (rule 1), gate first: with the bit
            // already set, no later reader can mistake the lowered count
            // for an open channel.
            mapped.claims().fetch_sub(1, Ordering::Relaxed);
            return Err(err);
        }

        // SAFETY: the claim reserved this span for itself, and the check
        // above put it inside the region. Other writers reserve spans
        // that never overlap, and the receiver reads no payload until it
        // sees the committed descriptor, which `finish` writes only by
        // taking this borrow.
        let content = unsafe {
            slice::from_raw_parts_mut(
                mapped.payloads.add(payload_offset as usize).as_ptr(),
                frame_size.get() as usize,
            )
        };
        Ok(FrameMut {
            mapped,
            slot_index,
            descriptor: SlotState::Committed { offset: payload_offset, len: frame_size }.encode(),
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
/// [`FrameMut::finish`] commits the frame; it is the only way to show the
/// payload to the receiver. Dropping the frame gives the claim up: the
/// slot stays unfinished and the receiver ignores it, exactly as if the
/// writer had died there. A writer that drops a frame and still performs
/// the operation it described breaks the rule this channel is built on —
/// write the record first, then do the thing it records.
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
    /// If the receiver sealed the channel and froze this slot first, the
    /// swap fails and the frame is dropped without a sound: this record
    /// raced the seal, and is meant to be left out either way.
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

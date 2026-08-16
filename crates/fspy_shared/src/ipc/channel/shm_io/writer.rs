//! The writer side: claim a frame, fill it, finish it.

use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::{Deref, DerefMut},
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{CLOSED, MappedLayout, SlotState, to_usize},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use from many threads and processes at once: each frame is
/// reserved atomically, filled in a span no one else can touch, and
/// published with one atomic write (the ordering contract in
/// [`super::layout`]).
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
    /// right either way — the receiver had already stopped collecting, or
    /// that same bit already makes the seal fail.
    #[error("the channel has been closed")]
    Closed,
    /// There was no room: the region is full, or the frame is longer than
    /// the `u32::MAX`-byte limit. The loss is already recorded — this
    /// claim set the CLOSED gate — so sealing the channel will fail and
    /// every later claim is refused.
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

    /// Claims a frame of exactly `frame_size` bytes: two compare-and-swap
    /// loops, one per counter (rule 1).
    ///
    /// The receiver cannot see the frame until [`FrameMut::finish`]
    /// commits it; dropping it instead gives the claim up, and the
    /// receiver ignores the slot exactly as if the writer had died. A
    /// claim that does not fit — the region is full, or `frame_size` is
    /// over `u32::MAX` — fails as [`ClaimError::Capacity`] after setting
    /// the CLOSED gate.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let mapped = self.mapped;

        // The loss report (rule 1): the gate marks the frames incomplete
        // and shuts the channel down for later claims.
        let report_loss = || {
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);
            ClaimError::Capacity
        };

        // A frame too long for the descriptor's 32-bit length field cannot
        // be described, so this conversion is the oversize check.
        let Ok(frame_size) = NonZeroU32::try_from(frame_size) else {
            return Err(report_loss());
        };
        // The payload space this claim takes, as the counter's `u64`.
        // Widening a 32-bit size never loses anything.
        let reservation = u64::from(frame_size.get());
        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot. The add refuses to wrap rather than wrapping: an overflowed
        // counter would hand out an offset inside the region that another
        // frame already owns.
        let Ok(payload_start) = mapped.payload_reserved().try_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |reserved| reserved.checked_add(reservation),
        ) else {
            return Err(report_loss());
        };
        let Some(payload_offset) =
            fitted_offset(payload_start, frame_size.get(), mapped.payload_len)
        else {
            return Err(report_loss());
        };

        // The claim counter shares its word with the gate, so the count
        // stops one short of it rather than carrying into it. That one
        // condition covers both refusals: a gate already set is above the
        // ceiling too.
        let claims =
            match mapped.claims().try_update(Ordering::Relaxed, Ordering::Relaxed, |claims| {
                (claims < CLOSED - 1).then(|| claims + 1)
            }) {
                Ok(claims) => claims,
                // Not a loss: the receiver had already stopped collecting when
                // this record was refused.
                Err(claims) if claims & CLOSED != 0 => return Err(ClaimError::Closed),
                // The count reached its ceiling, which takes 2^63 claims.
                Err(_) => return Err(report_loss()),
            };
        let slot_index = to_usize(claims);
        if slot_index >= SLOTS {
            return Err(report_loss());
        }

        // SAFETY: the claim reserved this span for itself, and the check
        // above put it inside the region. Other writers reserve spans
        // that never overlap, and the receiver reads no payload until it
        // sees the committed descriptor, which `finish` writes only by
        // taking this borrow.
        let content = unsafe {
            slice::from_raw_parts_mut(
                mapped.payload_start.add(to_usize(payload_offset)).as_ptr(),
                to_usize(frame_size.get()),
            )
        };
        Ok(FrameMut {
            slot: &self.mapped.table()[slot_index],
            slot_to_commit: SlotState::Committed { offset: payload_offset, len: frame_size }
                .encode(),
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

/// The offset of a `len`-byte frame reserved at `start`, or `None` when it
/// does not fit: the start must be small enough for a descriptor's 32-bit
/// offset, and the frame must end inside the payload area. Both are
/// checked, so if something else scribbled on the counter the claim fails
/// rather than wrapping around into a span outside the region.
fn fitted_offset(start: u64, len: u32, payload_len: u32) -> Option<u32> {
    let offset = u32::try_from(start).ok()?;
    let end = offset.checked_add(len)?;
    (end <= payload_len).then_some(offset)
}

/// An exclusively owned, claimed-but-unpublished frame.
///
/// [`FrameMut::finish`] commits the frame; it is the only way to show the
/// payload to the receiver. Dropping the frame gives the claim up: the
/// slot stays unfinished and the receiver ignores it, exactly as if the
/// writer had died there. A writer that drops a frame and still performs
/// the operation it described breaks the rule this channel is built on —
/// write the record first, then do the thing it records.
#[derive(Debug)]
pub struct FrameMut<'a> {
    slot: &'a AtomicU64,
    slot_to_commit: u64,
    content: &'a mut [u8],
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
    /// A receiver that already sealed the channel may or may not show this
    /// frame: it reads the table when asked, so what it reports depends on
    /// whether the descriptor is there yet. Either answer is truthful —
    /// the operation this record describes had not happened when the
    /// receiver drew its line.
    pub fn finish(self) {
        // Rule 2: `Release` orders every payload write before the
        // descriptor. This writer owns the slot, so a store is enough.
        self.slot.store(self.slot_to_commit, Ordering::Release);
    }
}

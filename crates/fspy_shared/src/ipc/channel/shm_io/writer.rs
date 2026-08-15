//! The writer side: claim a frame, fill it, finish it.

use std::{
    fmt,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    slice,
    sync::atomic::Ordering,
};

use super::{
    AsRawSlice,
    layout::{self, CLOSED, MappedLayout},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use across threads and processes at the same time: frames are
/// reserved with atomic operations, filled in uniquely owned payload spans,
/// and published with an atomic commit (see the ordering contract above).
pub struct ShmWriter<M> {
    /// Owns the region the views point into; dropped with the writer.
    #[cfg_attr(any(not(test), miri), expect(dead_code, reason = "held to keep the region alive"))]
    mem: M,
    mapped: MappedLayout,
}

// SAFETY: the writer touches the region only through the protocol's
// atomics, which synchronize access from any thread; the stored views
// point into the mapping's stable, independently owned target, not into
// the writer value itself.
unsafe impl<M: Send> Send for ShmWriter<M> {}
// SAFETY: see the `Send` impl; the writer's shared-reference API is
// internally synchronized by the protocol.
unsafe impl<M: Sync> Sync for ShmWriter<M> {}

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimError {
    /// The CLOSED gate was set: the receiver closed the channel, or an
    /// earlier failed claim condemned it. Skipping the record is sound
    /// either way — it is outside the receiver's boundary, or the same
    /// bit already makes the receiver report the channel incomplete.
    #[error("the channel has been closed")]
    Closed,
    /// The claim was refused for space: the region was full, or the frame
    /// was larger than the `i32::MAX`-byte frame limit. The loss is
    /// already recorded — this claim set the CLOSED gate — so the channel
    /// will report itself incomplete and refuse further claims.
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
    /// supported range (see [`MappedLayout::new`]).
    pub unsafe fn new(mem: M) -> Self {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid and protocol-governed for the writer's lifetime —
        // and so for every use of the views, which are stored in and
        // dropped with the writer.
        let mapped = unsafe { MappedLayout::new(mem.as_raw_slice()) };
        Self { mem, mapped }
    }

    /// Whether the CLOSED gate is set: the receiver closed the channel,
    /// or an earlier failed claim condemned it.
    pub fn is_closed(&self) -> bool {
        self.mapped.header().claims.load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Claims a frame of exactly `frame_size` bytes.
    ///
    /// The frame is invisible to the receiver until [`FrameMut::finish`]
    /// commits it. Dropping the frame without finishing abandons the claim:
    /// the receiver ignores the slot, exactly as if the writer had died.
    /// Frames larger than `i32::MAX` bytes are refused as
    /// [`ClaimError::Capacity`].
    ///
    /// Wait-free: two `fetch_add`s, no retry loop (rule 1). A claim that
    /// does not fit fails after setting the CLOSED gate: the receiver
    /// learns a record was lost, and later claims are refused — their
    /// records would ride a result the receiver must already reject.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let mapped = self.mapped;
        let payload_len = frame_size.get();

        // Reports that this claim's record was lost, before the writer
        // moves on (rule 1): the gate makes the receiver report the
        // channel incomplete, and condemns further claims.
        let report_loss = || {
            mapped.header().claims.fetch_or(CLOSED, Ordering::Relaxed);
            ClaimError::Capacity
        };

        // No descriptor can describe a payload this long; refuse it before
        // touching the counters, so the channel keeps working for every
        // record after it.
        if payload_len > layout::MAX_PAYLOAD_LEN {
            return Err(report_loss());
        }
        let reserved_len = layout::reserved_payload_len(payload_len);

        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot. A failed reservation stays counted — overshoot is harmless
        // because the counter is not what locates payloads (descriptors are)
        // and a `u64` cannot realistically wrap.
        let payload_start =
            mapped.header().payload_reserved.fetch_add(reserved_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(reserved_len as u64);
        if payload_end.is_none_or(|end| end > mapped.payloads.len() as u64) {
            return Err(report_loss());
        }
        let payload_start = usize::try_from(payload_start).expect("bounded by the payload region");

        let claims = mapped.header().claims.fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            // Not a loss: a record refused after close describes an
            // operation performed outside the channel's boundary.
            return Err(ClaimError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= mapped.table().len() {
            return Err(report_loss());
        }

        // SAFETY: the claim reserved
        // `[payload_start, payload_start + reserved_len)` — inside the
        // payload region by the capacity check above — exclusively for this
        // frame: other writers reserve disjoint spans, and the receiver
        // never reads a payload before observing its committed descriptor,
        // which `finish` publishes only when it consumes this borrow.
        let content = unsafe {
            slice::from_raw_parts_mut(mapped.payloads.cast::<u8>().add(payload_start), payload_len)
        };
        Ok(FrameMut {
            mapped,
            slot_index,
            descriptor: layout::committed(mapped.payload_base() + payload_start, payload_len),
            content,
        })
    }

    // Unwrap `self` and return the underlying memory.
    #[cfg(all(test, not(miri)))]
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
    mapped: MappedLayout,
    slot_index: usize,
    descriptor: u64,
    content: &'a mut [u8],
}

impl fmt::Debug for FrameMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    /// first, the swap fails and the frame is silently discarded: the
    /// record belongs to the close race and is intentionally excluded
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

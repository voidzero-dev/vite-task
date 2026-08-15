//! The receiver side: closing the channel and borrowing committed frames.
//!
//! Closing never waits for writers. The close boundary is a snapshot of the
//! claim counter; unfinished slots inside the snapshot are atomically
//! frozen, committed descriptors are validated, and the CLOSED gate is set
//! so stragglers stop claiming. No payload byte is read or copied here:
//! [`Frames`] keeps the mapping alive and hands out borrows of the
//! validated committed spans on demand.
//!
//! Those borrows are sound because of the protocol, not despite it:
//! a committed span is never written again (committing consumes the
//! writer's frame), every borrow covers exactly one validated committed
//! span, and everything a live writer may still touch — counters, slots,
//! its own claimed or aborted spans — is disjoint from every committed
//! span. This rests on the constructor contract that the region is accessed
//! only through this protocol; a process scribbling outside the protocol is
//! outside the trust model.

use std::{fmt, slice};

use super::{
    AsRawSlice,
    layout::PayloadSpan,
    slot::{self, SlotState},
    state::SharedState,
};

/// The committed frames of a closed channel: validated spans borrowed from
/// the mapping, which stays alive inside this value. Dropping it releases
/// the mapping.
pub struct Frames<M> {
    mem: M,
    spans: Vec<PayloadSpan>,
    complete: bool,
}

impl<M: AsRawSlice> Frames<M> {
    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        let base = self.mem.as_raw_slice().cast::<u8>().cast_const();
        self.spans.iter().map(move |span| {
            // SAFETY: `close` validated the span against this mapping's
            // layout, and a committed span is immutable for the mapping's
            // lifetime (see the module docs), so the shared borrow is valid
            // for as long as `self` lives.
            unsafe { slice::from_raw_parts(base.add(span.offset), span.len) }
        })
    }

    /// Whether the trace contains every reported access.
    ///
    /// False when a live writer lost a record before the channel closed
    /// (capacity exhaustion or an abandoned frame): the trace then
    /// under-reports the run's accesses and must not back a cache entry.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

impl<M> fmt::Debug for Frames<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frames")
            .field("frames", &self.spans.len())
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// A trace whose shared-memory metadata could not have been produced by this
/// protocol. The mapping was corrupted; the trace is unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// Closes the channel and returns the committed frames as borrows of the
/// mapping, which moves into the returned [`Frames`].
///
/// Never blocks on writers: writers admitted before the snapshot race per
/// slot, and each raced slot independently ends up committed (included) or
/// aborted (excluded). Claims after the snapshot land in slots this pass
/// never visits until the CLOSED gate — set before returning — stops them.
/// See the crate-level protocol docs in [`super`].
///
/// # Safety
///
/// Same contract as [`super::ShmWriter::new`]: `mem` must be a stable, valid
/// pointer to the whole region, zero-initialized at creation and accessed
/// only through this protocol.
///
/// # Panics
///
/// Panics when the region is not word-aligned or its size is outside the
/// supported range (see [`SharedState::borrow`]) — a broken caller, not
/// corrupt shared data, which is reported as [`ProtocolError`] instead.
pub(super) unsafe fn close<M: AsRawSlice>(mem: M) -> Result<Frames<M>, ProtocolError> {
    let spans;
    let complete;
    {
        // SAFETY: forwarded from this function's contract; the raw slice
        // stays valid while `mem` is borrowed here and beyond, since `mem`
        // moves into the returned `Frames`.
        let state = unsafe { SharedState::borrow(mem.as_raw_slice()) };

        // The close boundary: claims at or before this snapshot are inside
        // it, later ones land in slots this pass never visits. The count is
        // clamped to the table capacity, so a counter inflated by failed
        // claims (or by a foreign scribble) degrades to a full-table sweep,
        // not an error.
        let slot_count = state.snapshot_claims();

        // Gate further claims. Cheap: the creator pre-faulted this page
        // where first touches are expensive. Claims racing between the
        // snapshot and this gate are dropped soundly (see the module docs
        // in `super`).
        state.close_claims();

        // Freeze pass: drive every admitted slot to a terminal state and
        // collect the committed spans. After this loop the snapshot's slice
        // of the descriptor table can no longer change — late writers lose
        // their commit race against `ABORTED`.
        spans = freeze_committed_spans(state, slot_count)?;

        // Read the incomplete flag only after freezing: a writer sets it
        // before performing an operation whose record was lost, so any flag
        // this load misses belongs to an operation performed after the
        // boundary (rule 1 in `state`'s ordering contract).
        complete = !state.is_incomplete();
    }

    Ok(Frames { mem, spans, complete })
}

fn freeze_committed_spans(
    state: SharedState<'_>,
    slot_count: usize,
) -> Result<Vec<PayloadSpan>, ProtocolError> {
    let mut spans = Vec::new();
    for slot_index in 0..slot_count {
        match slot::decode(state.freeze(slot_index)) {
            SlotState::Aborted => {}
            SlotState::Committed { payload_offset, payload_len } => {
                let span = PayloadSpan::validate(state.mapping_len(), payload_offset, payload_len)
                    .ok_or(ProtocolError::CorruptDescriptor { slot_index })?;
                spans.push(span);
            }
            // `freeze` only returns terminal values, so `Unfinished` is
            // unreachable and grouped with the corrupt case.
            SlotState::Unfinished | SlotState::Corrupt => {
                return Err(ProtocolError::CorruptDescriptor { slot_index });
            }
        }
    }
    Ok(spans)
}

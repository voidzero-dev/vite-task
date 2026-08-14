//! The receiver side: closing the channel and collecting committed frames.
//!
//! Closing never waits for writers. It prevents new claims, atomically
//! freezes every slot that never committed, validates the committed
//! descriptors, and copies their payloads out of the shared mapping.
//!
//! The copy is deliberate: the mapping stays writable in every traced
//! process, so this module never creates a reference into shared memory
//! whose validity would depend on another process's compliance. Committed
//! spans are read with atomic loads (see [`super::state`]) into an owned
//! buffer, and everything downstream parses the private copy.

use std::ops::Range;

use super::{
    layout::{self, PayloadSpan},
    slot::{self, SlotState},
    state::SharedState,
};

/// The committed frames of a closed channel, copied out of shared memory.
#[derive(Debug)]
pub struct Frames {
    bytes: Vec<u8>,
    bounds: Vec<Range<usize>>,
    complete: bool,
}

impl Frames {
    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.bounds.iter().map(|bounds| &self.bytes[bounds.clone()])
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

/// A trace whose shared-memory metadata could not have been produced by this
/// protocol. The mapping was corrupted; the trace is unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    #[error("corrupt shared-memory allocator word")]
    CorruptAllocator,
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// Closes the channel and collects the committed frames.
///
/// Never blocks on writers: writers admitted before the close race per slot,
/// and each raced slot independently ends up committed (included) or aborted
/// (excluded). See the crate-level protocol docs in [`super`].
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
pub(super) unsafe fn close(mem: *mut [u8]) -> Result<Frames, ProtocolError> {
    // SAFETY: forwarded from this function's contract.
    let state = unsafe { SharedState::borrow(mem) };

    // Snapshot the admitted slots and stop admitting new ones. Claims and
    // close totally order on the allocator word: every claim admitted before
    // this operation is counted, every later one fails with `Closed`.
    let snapshot = state.close();
    if !snapshot.is_valid_for(state.mapping_len()) {
        return Err(ProtocolError::CorruptAllocator);
    }
    let slot_count = snapshot.slot_count();
    let table_end = layout::table_end(slot_count);

    // Freeze pass: drive every admitted slot to a terminal state and collect
    // the committed spans. After this loop the descriptor table can no
    // longer change — late writers lose their commit race against `ABORTED`.
    let mut spans = Vec::new();
    let mut payload_total = 0usize;
    for slot_index in 0..slot_count {
        match slot::decode(state.freeze(slot_index)) {
            SlotState::Aborted => {}
            SlotState::Committed { payload_offset, payload_len } => {
                let span = PayloadSpan::validate(
                    state.mapping_len(),
                    table_end,
                    payload_offset,
                    payload_len,
                )
                .ok_or(ProtocolError::CorruptDescriptor { slot_index })?;
                spans.push(span);
                payload_total += span.len;
            }
            // `freeze` only returns terminal values, so `Unfinished` is
            // unreachable and grouped with the corrupt case.
            SlotState::Unfinished | SlotState::Corrupt => {
                return Err(ProtocolError::CorruptDescriptor { slot_index });
            }
        }
    }

    // Copy pass: move the committed payloads into an owned buffer.
    let mut bytes = Vec::with_capacity(payload_total);
    let mut bounds = Vec::with_capacity(spans.len());
    for span in spans {
        let start = bytes.len();
        state.read_payload(span, &mut bytes);
        bounds.push(start..bytes.len());
    }

    // Re-read the incomplete flag only after freezing: a writer sets it
    // before performing an operation whose record was lost, so any flag this
    // load misses belongs to an operation performed after close — outside
    // the tracking boundary (rule 1 in `state`'s ordering contract).
    let complete = !state.reload().is_incomplete();

    Ok(Frames { bytes, bounds, complete })
}

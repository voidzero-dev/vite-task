//! The reader side: seal the channel, then iterate the committed frames.
//!
//! Sealing never waits for writers, and no payload byte is read or
//! copied: the reader keeps the mapping alive and hands out each
//! committed span on demand. Those borrows are safe because nothing
//! writes to a committed span again (committing uses up the writer's
//! frame) and it never overlaps what a live writer may still touch.
//! All of this assumes the promise made at attach — that the region is
//! touched only through this protocol. A process that scribbles on it
//! some other way breaks that promise, and this code does not defend
//! against it.

use std::{
    fmt, slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{self, CLOSED, FROZEN, MappedLayout, SlotState, to_usize},
};

/// Why a channel could not be sealed into readable frames.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    /// The mapping cannot hold the protocol at all (see
    /// [`MappedLayout::new`]).
    #[error("the shared-memory region cannot host the channel")]
    UnsupportedRegion,
    /// A descriptor no correct writer could have written: something
    /// corrupted the region, and its frames cannot be used.
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// A reader over the committed frames of a sealed channel, serving them
/// straight out of the mapping, which stays alive inside this value and
/// is released when the reader drops. It holds no buffer of its own:
/// iterating re-reads the frozen descriptor table, so sealing a channel
/// allocates nothing.
pub struct ShmReader<M, const SLOTS: usize> {
    /// Where the parts of the owned region are.
    mapped: MappedLayout<SLOTS>,
    /// Owns the region the pointers point into. Declared after them:
    /// fields drop in order, and the borrower must go first.
    _mem: M,
    /// Length of the frozen prefix of the descriptor table.
    slot_count: usize,
    /// Committed frames in that prefix.
    frames: usize,
    complete: bool,
}

// SAFETY: the reader reads only the counters, frozen slots, and committed
// spans that nothing writes to any more; the stored pointers point into
// the mapped memory, which is owned separately and does not move, not
// into the reader itself.
unsafe impl<M: Send, const SLOTS: usize> Send for ShmReader<M, SLOTS> {}
// SAFETY: see the `Send` impl.
unsafe impl<M: Sync, const SLOTS: usize> Sync for ShmReader<M, SLOTS> {}

impl<M: AsRawSlice, const SLOTS: usize> ShmReader<M, SLOTS> {
    /// Seals the channel — no further records — and returns the reader
    /// of its committed frames.
    ///
    /// Never waits for writers. A writer that got in before the snapshot
    /// races this pass for its own slot and ends up either committed
    /// (kept) or frozen (skipped); a claim taken after the snapshot lands
    /// in a slot this pass never looks at, until the CLOSED gate — set
    /// before this returns — stops the claims for good.
    ///
    /// # Safety
    ///
    /// Same contract as [`ShmWriter::new`](super::ShmWriter::new):
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the reader's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    ///
    /// # Errors
    ///
    /// [`ProtocolError`]: the mapping cannot hold the protocol, or it
    /// contains something no correct writer could have written — the
    /// region was corrupted and its frames cannot be used.
    pub unsafe fn seal(mem: M) -> Result<Self, ProtocolError> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid for as long as the reader lives — and so for every
        // use of the pointers, which are stored in the reader and dropped
        // with it.
        let Some(mapped) = (unsafe { MappedLayout::new(mem.as_raw_slice()) }) else {
            return Err(ProtocolError::UnsupportedRegion);
        };
        let slot_count;
        let mut frames = 0;
        let complete;
        {
            // The seal boundary (rule 1): claims at or before this
            // snapshot are in, later ones land in slots this pass never
            // looks at. Clamped, so a counter reading higher than the
            // table just means sweeping the whole table, not an error.
            let claims = mapped.claims().load(Ordering::Relaxed);
            slot_count = to_usize(claims & !CLOSED).min(SLOTS);
            // The same load answers the other question: a gate already
            // set before the boundary means a record was lost — or that
            // someone sealed earlier, and a second seal cannot promise
            // anything about records refused in between.
            complete = claims & CLOSED == 0;

            // Shut the gate, so late writers stop claiming slots and
            // touching new pages. A claim that slips in between is
            // dropped safely (rule 1).
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);

            // Freeze pass: give every slot up to the boundary its final
            // value, and check the committed ones. Afterwards this part
            // of the table can never change — a late commit loses to
            // `FROZEN` — so iterating just re-reads it, copying and
            // allocating nothing.
            for slot_index in 0..slot_count {
                // Rule 3: `Acquire` on failure means that if this slot
                // is committed, its payload bytes are all visible.
                let Err(bits) = mapped.table()[slot_index].compare_exchange(
                    layout::UNFINISHED,
                    FROZEN,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) else {
                    // The receiver won the race: the unfinished slot is
                    // frozen and stays ignored.
                    continue;
                };
                match SlotState::decode(bits) {
                    // Frozen by an earlier seal, or scribbled on without
                    // publishing anything: no frame either way.
                    SlotState::Unfinished => {}
                    SlotState::Committed { offset, len } => {
                        // A span no correct writer could have written
                        // fails the whole channel.
                        let end = offset.checked_add(len.get());
                        if end.is_none_or(|end| end > mapped.payload_len) {
                            return Err(ProtocolError::CorruptDescriptor { slot_index });
                        }
                        frames += 1;
                    }
                }
            }
        }

        Ok(Self { mapped, _mem: mem, slot_count, frames, complete })
    }

    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> Iter<'_, SLOTS> {
        self.into_iter()
    }

    /// Whether every record a writer published made it in.
    ///
    /// False when a claim failed before the seal — no room left, or an
    /// oversized frame. That record is gone, so the frames say less than
    /// the writers actually did. Anyone who needs the full picture must
    /// throw them away.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

impl<M, const SLOTS: usize> fmt::Debug for ShmReader<M, SLOTS> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShmReader")
            .field("frames", &self.frames)
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// Iterator over a [`ShmReader`]'s committed frames, in claim order.
pub struct Iter<'a, const SLOTS: usize> {
    /// Where the payload area is, for turning descriptors into spans.
    mapped: MappedLayout<SLOTS>,
    /// The frozen slots this iterator has not reached yet.
    table: &'a [AtomicU64],
    /// Committed frames not yet yielded.
    remaining: usize,
}

impl<'a, const SLOTS: usize> Iterator for Iter<'a, SLOTS> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((slot, rest)) = self.table.split_first() {
            self.table = rest;
            // The slot can never change again, so this plain load reads
            // what the freeze pass saw; whatever brought the reader to
            // this thread brought that pass's `Acquire` along (rule 3).
            let bits = slot.load(Ordering::Relaxed);
            // An unfinished slot published nothing. A span outside the
            // region cannot appear here — `seal` fails the channel on
            // one — but checking lets the `unsafe` below stand on its
            // own.
            let SlotState::Committed { offset, len } = SlotState::decode(bits) else {
                continue;
            };
            if offset.checked_add(len.get()).is_none_or(|end| end > self.mapped.payload_len) {
                continue;
            }
            self.remaining -= 1;
            // SAFETY: the span is inside the region (checked above) and
            // nothing writes to it any more; the reader borrowed for `'a`
            // keeps the mapping alive.
            return Some(unsafe {
                slice::from_raw_parts(
                    self.mapped.payloads.add(offset as usize).as_ptr().cast_const(),
                    len.get() as usize,
                )
            });
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, M: AsRawSlice, const SLOTS: usize> IntoIterator for &'a ShmReader<M, SLOTS> {
    type IntoIter = Iter<'a, SLOTS>;
    type Item = &'a [u8];

    fn into_iter(self) -> Iter<'a, SLOTS> {
        Iter {
            mapped: self.mapped,
            table: &self.mapped.table()[..self.slot_count],
            remaining: self.frames,
        }
    }
}

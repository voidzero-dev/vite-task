//! The reader side: seal the channel, then iterate the committed frames.
//!
//! Sealing never waits for writers, and no payload byte is read or
//! copied: the reader keeps the mapping alive and lends out each
//! committed span on demand. The borrows are sound because a committed
//! span is never written again (committing consumes the writer's frame)
//! and is disjoint from everything a live writer may still touch. This
//! rests on the attach contract that the region is accessed only through
//! this protocol; a process scribbling outside it is outside the trust
//! model.

use std::{
    fmt, slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{self, CLOSED, FROZEN, MappedLayout, SlotState},
};

/// Why a channel could not be sealed into readable frames.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    /// The mapping cannot host the protocol at all (see
    /// [`MappedLayout::new`]).
    #[error("the shared-memory region cannot host the channel")]
    UnsupportedRegion,
    /// A descriptor that no correct writer could have committed: the
    /// region was corrupted, and its frames are unusable.
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// A reader over the committed frames of a sealed channel, serving them
/// straight out of the mapping, which stays alive inside this value and
/// is released when the reader drops. It holds no buffer: iteration
/// re-reads the frozen descriptor table, so closing allocates nothing.
pub struct ShmReader<M, const SLOTS: usize> {
    /// The layout mapped onto the owned region.
    mapped: MappedLayout<SLOTS>,
    /// Owns the region the views point into. Declared after them: fields
    /// drop in order, and what borrows must die before what is borrowed.
    _mem: M,
    /// Length of the frozen prefix of the descriptor table.
    slot_count: usize,
    /// Committed frames in that prefix.
    frames: usize,
    complete: bool,
}

// SAFETY: the reader reads only the header atomics, frozen slots, and
// immutable committed spans; the stored views point into the mapping's
// stable, independently owned target, not into the reader value itself.
unsafe impl<M: Send, const SLOTS: usize> Send for ShmReader<M, SLOTS> {}
// SAFETY: see the `Send` impl.
unsafe impl<M: Sync, const SLOTS: usize> Sync for ShmReader<M, SLOTS> {}

impl<M: AsRawSlice, const SLOTS: usize> ShmReader<M, SLOTS> {
    /// Seals the channel — no further records — and returns the reader
    /// of its committed frames.
    ///
    /// Never blocks on writers: writers admitted before the snapshot race
    /// per slot, ending committed (included) or frozen (excluded); claims
    /// after the snapshot land in slots this pass never visits, until the
    /// CLOSED gate — set before this returns — stops them.
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
    /// [`ProtocolError`]: the mapping cannot host the protocol, or its
    /// metadata could not have been produced by a correct writer — the
    /// region was corrupted and its frames are unusable.
    pub unsafe fn seal(mem: M) -> Result<Self, ProtocolError> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid for the reader's lifetime — and so for every use of
        // the views, which are stored in and dropped with the reader.
        let Some(mapped) = (unsafe { MappedLayout::new(mem.as_raw_slice()) }) else {
            return Err(ProtocolError::UnsupportedRegion);
        };
        let slot_count;
        let mut frames = 0;
        let complete;
        {
            // The seal boundary (rule 1): claims at or before this
            // snapshot are in; later ones land in slots this pass never
            // visits. Clamped, so an inflated counter degrades to a
            // full-table sweep, not an error.
            let claims = mapped.claims().load(Ordering::Relaxed);
            slot_count = usize::try_from(claims & !CLOSED).unwrap_or(usize::MAX).min(SLOTS);
            // The same load carries the verdict: a gate set before the
            // boundary is a loss report — or an earlier seal, and a
            // re-seal cannot vouch for records refused since then.
            complete = claims & CLOSED == 0;

            // Gate further claims, so stragglers stop claiming and
            // materializing pages. Claims racing in between are dropped
            // soundly (rule 1).
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);

            // Freeze pass: drive every admitted slot terminal and
            // validate committed descriptors. Afterwards this prefix of
            // the table can never change — late commits lose to `FROZEN`
            // — so iteration re-reads it: nothing copied, nothing
            // allocated.
            for slot_index in 0..slot_count {
                // Rule 3: `Acquire` on failure makes a committed payload
                // visible.
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
                    // Frozen by an earlier seal, or a scribble that
                    // published nothing: no frame either way.
                    SlotState::Unfinished => {}
                    SlotState::Committed { offset, len } => {
                        // A span no correct writer could have committed
                        // fails the whole channel.
                        if offset as usize + len.get() as usize > mapped.payloads.len() {
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
    /// False when a claim failed before the seal — out of space, or an
    /// oversized frame: the record was lost, and the frames under-report
    /// what writers went on to do. Consumers that need completeness must
    /// reject them.
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
    /// The layout the spans decode against and point into.
    mapped: MappedLayout<SLOTS>,
    /// The not-yet-visited part of the table's frozen prefix.
    table: &'a [AtomicU64],
    /// Committed frames not yet yielded.
    remaining: usize,
}

impl<'a, const SLOTS: usize> Iterator for Iter<'a, SLOTS> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((slot, rest)) = self.table.split_first() {
            self.table = rest;
            // The slot is terminal, so this plain load reads what the
            // freeze pass saw; whatever carried the reader to this thread
            // carried the freeze pass's `Acquire` visibility too (rule 3).
            let bits = slot.load(Ordering::Relaxed);
            // Unfinished published nothing. Out-of-bounds cannot appear
            // — `seal` failed the channel on it — but checking keeps the
            // `unsafe` below locally justified.
            let SlotState::Committed { offset, len } = SlotState::decode(bits) else {
                continue;
            };
            let (offset, len) = (offset as usize, len.get() as usize);
            if offset + len > self.mapped.payloads.len() {
                continue;
            }
            self.remaining -= 1;
            // SAFETY: the span is in bounds (checked above) and
            // immutable for the mapping's lifetime; the reader borrowed
            // for `'a` keeps the mapping alive and mapped.
            return Some(unsafe {
                slice::from_raw_parts(
                    self.mapped.payloads.cast::<u8>().cast_const().add(offset),
                    len,
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

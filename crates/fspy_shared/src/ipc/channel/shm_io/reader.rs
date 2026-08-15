//! The reader side: close the channel, then iterate the committed frames.
//!
//!
//! Closing never waits for writers, and no payload byte is read or copied:
//! the reader keeps the mapping alive and hands out borrows of the validated
//! committed spans on demand. Those borrows are sound because of the
//! protocol, not despite it: a committed span is never written again
//! (committing consumes the writer's frame), every borrow covers exactly one
//! validated committed span, and everything a live writer may still touch —
//! counters, slots, its own claimed or abandoned spans — is disjoint from
//! every committed span. This rests on the constructor contract that the
//! region is accessed only through this protocol; a process scribbling
//! outside the protocol is outside the trust model.

use std::{
    fmt, slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{self, CLOSED, MappedLayout},
};

/// Shared-memory metadata that could not have been produced by this
/// protocol. The region was corrupted; its frames are unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// A reader over the committed frames of a closed channel, serving them
/// straight out of the mapping, which stays alive inside this value and
/// is released when the reader drops. It holds no buffer: iteration
/// re-reads the frozen descriptor table, so closing allocates nothing.
pub struct ShmReader<M> {
    /// Owns the region the views point into; dropped with the reader.
    #[expect(dead_code, reason = "held to keep the region alive")]
    mem: M,
    /// The layout mapped onto the owned region.
    mapped: MappedLayout,
    /// Length of the frozen prefix of the descriptor table.
    slot_count: usize,
    /// Committed frames in that prefix.
    frames: usize,
    complete: bool,
}

// SAFETY: the reader reads only the header atomics, frozen slots, and
// immutable committed spans; the stored views point into the mapping's
// stable, independently owned target, not into the reader value itself.
unsafe impl<M: Send> Send for ShmReader<M> {}
// SAFETY: see the `Send` impl.
unsafe impl<M: Sync> Sync for ShmReader<M> {}

impl<M: AsRawSlice> ShmReader<M> {
    /// Closes the channel over a shared-memory region and returns the
    /// reader of its committed frames.
    ///
    /// Never blocks on writers: writers admitted before the snapshot race
    /// per slot, and each raced slot independently ends up committed
    /// (included) or aborted (excluded). Claims after the snapshot land in
    /// slots this pass never visits until the CLOSED gate — set before
    /// this returns — stops them. See the protocol docs at the top of this
    /// module.
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
    /// [`ProtocolError`] when the shared-memory metadata could not have
    /// been produced by a correct writer; the region was corrupted and its
    /// frames are unusable.
    ///
    /// # Panics
    ///
    /// Panics when the region is not `u64`-aligned or its size is outside
    /// the supported range (see [`MappedLayout::new`]).
    pub unsafe fn close(mem: M) -> Result<Self, ProtocolError> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid for the reader's lifetime — and so for every use of
        // the views, which are stored in and dropped with the reader.
        let mapped = unsafe { MappedLayout::new(mem.as_raw_slice()) };
        let slot_count;
        let mut frames = 0;
        let complete;
        {
            // The close boundary (rule 1): claims at or before this
            // snapshot are inside it, later ones land in slots this pass
            // never visits. The count is clamped to the table capacity, so
            // a counter inflated by failed claims (or by a foreign
            // scribble) degrades to a full-table sweep, not an error.
            let claims = mapped.header().claims.load(Ordering::Relaxed);
            slot_count =
                usize::try_from(claims & !CLOSED).unwrap_or(usize::MAX).min(mapped.table().len());
            // The same load carries the completeness verdict: a gate set
            // before this boundary is a failed claim's loss report — or an
            // earlier close, and a re-close cannot vouch for records
            // refused since then (rule 1).
            complete = claims & CLOSED == 0;

            // Gate further claims, so stragglers stop claiming (and
            // materializing pages) forever. Cheap: the creator pre-faulted
            // this page where first touches are expensive. Claims racing
            // between the snapshot and this gate are dropped soundly (see
            // the module docs above).
            mapped.header().claims.fetch_or(CLOSED, Ordering::Relaxed);

            // Freeze pass: drive every admitted slot to a terminal state
            // and validate the committed descriptors. After this loop the
            // snapshot's slice of the descriptor table can no longer change
            // — late writers lose their commit race against `ABORTED` — so
            // iteration re-reads the table instead of snapshotting it:
            // nothing is copied or allocated.
            for slot_index in 0..slot_count {
                // Rule 3: `Acquire` on failure makes a committed payload
                // visible.
                let Err(bits) = mapped.table()[slot_index].compare_exchange(
                    layout::UNFINISHED,
                    layout::ABORTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) else {
                    // The receiver won the race: the unfinished slot is now
                    // aborted and stays ignored.
                    continue;
                };
                if bits == layout::ABORTED {
                    // Aborted by an earlier close over the same region;
                    // still ignored.
                    continue;
                }
                // Any other terminal value must be a committed descriptor
                // with a valid span; a foreign scribble fails the decode.
                if layout::decode(mapped.len, bits).is_none() {
                    return Err(ProtocolError::CorruptDescriptor { slot_index });
                }
                frames += 1;
            }
        }

        Ok(Self { mem, mapped, slot_count, frames, complete })
    }

    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> Iter<'_> {
        self.into_iter()
    }

    /// Whether every record a writer published made it in.
    ///
    /// False when a claim failed before the channel closed — the region
    /// was out of space, or a frame exceeded the frame limit: its record
    /// was lost, and the frames under-report what writers went on to do.
    /// Consumers that need completeness must reject them.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

impl<M> fmt::Debug for ShmReader<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShmReader")
            .field("frames", &self.frames)
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// Iterator over a [`ShmReader`]'s committed frames, in claim order.
pub struct Iter<'a> {
    /// Base address of the mapping the descriptors' spans point into.
    base: *const u8,
    /// The not-yet-visited part of the table's frozen prefix.
    table: &'a [AtomicU64],
    /// Mapping length, which decoding validates descriptors against.
    mapping_len: usize,
    /// Committed frames not yet yielded.
    remaining: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((slot, rest)) = self.table.split_first() {
            self.table = rest;
            // The slot is terminal (`close` froze it), so this plain load
            // reads the same value the freeze pass saw; the transfer that
            // carried the reader to this thread carried the freeze pass's
            // `Acquire` payload visibility with it (rule 3).
            let bits = slot.load(Ordering::Relaxed);
            // `None` is an aborted slot: nothing was published. Corrupt
            // values cannot appear — `close` already failed the channel on
            // them — so every decoded span is one `close` validated.
            let Some(span) = layout::decode(self.mapping_len, bits) else {
                continue;
            };
            self.remaining -= 1;
            // SAFETY: `close` validated the span against the mapping's
            // layout, and a committed span is immutable for the mapping's
            // lifetime (see the section comment above); the reader
            // borrowed for `'a` keeps the mapping alive and mapped.
            return Some(unsafe { slice::from_raw_parts(self.base.add(span.offset), span.len) });
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, M: AsRawSlice> IntoIterator for &'a ShmReader<M> {
    type IntoIter = Iter<'a>;
    type Item = &'a [u8];

    fn into_iter(self) -> Iter<'a> {
        Iter {
            base: self.mapped.base(),
            table: &self.mapped.table()[..self.slot_count],
            mapping_len: self.mapped.len,
            remaining: self.frames,
        }
    }
}

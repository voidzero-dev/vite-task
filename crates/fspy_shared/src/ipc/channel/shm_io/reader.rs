//! The reader side: seal the channel, then iterate the committed frames.
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

/// The terminal value the freeze pass installs in an unfinished slot: the
/// aborted bit of the slot codec ([`layout`]).
const ABORTED: u64 = 1 << 63;

/// A validated payload byte range: the witness that offset arithmetic on this
/// span cannot leave the payload region.
///
/// Constructing a `PayloadSpan` through [`PayloadSpan::validate`] is the
/// single validation point for descriptor metadata read back from shared
/// memory; code holding a span may rely on its bounds without re-checking.
#[derive(Clone, Copy, Debug)]
struct PayloadSpan {
    /// Byte offset of the payload from the start of the payload region.
    offset: usize,
    /// Exact (unpadded) byte length of the payload.
    len: usize,
}

impl PayloadSpan {
    /// Validates a committed descriptor's payload range against a payload
    /// region of `payload_region_len` bytes. Returns `None` if the range
    /// could not have been produced by a correct writer.
    const fn validate(payload_region_len: usize, offset: usize, len: usize) -> Option<Self> {
        if len == 0 || len > layout::MAX_PAYLOAD_LEN {
            return None;
        }
        // `offset` and `len` come from 32-bit descriptor fields, so this
        // sum cannot overflow `usize`.
        if offset + len > payload_region_len {
            return None;
        }
        Some(Self { offset, len })
    }
}

/// Decodes a slot value read back from shared memory as a committed
/// descriptor (the slot codec in [`layout`]). Returns `None` for any value
/// that is not one a correct writer could have committed for a payload
/// region of `payload_region_len` bytes: an unfinished `0` decodes a zero
/// length, and any value carrying the aborted bit decodes a length beyond
/// the frame limit, so both fail validation exactly like a scribble.
const fn decode(payload_region_len: usize, bits: u64) -> Option<PayloadSpan> {
    PayloadSpan::validate(
        payload_region_len,
        (bits & layout::OFFSET_MAX) as usize,
        (bits >> layout::LEN_SHIFT) as usize,
    )
}

/// A descriptor that no correct writer could have committed: the region
/// was corrupted, and its frames are unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
#[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
pub struct ProtocolError {
    pub slot_index: usize,
}

/// A reader over the committed frames of a sealed channel, serving them
/// straight out of the mapping, which stays alive inside this value and
/// is released when the reader drops. It holds no buffer: iteration
/// re-reads the frozen descriptor table, so closing allocates nothing.
pub struct ShmReader<M, const SLOTS: usize> {
    /// Owns the region the views point into; dropped with the reader.
    #[expect(dead_code, reason = "held to keep the region alive")]
    mem: M,
    /// The layout mapped onto the owned region.
    mapped: MappedLayout<SLOTS>,
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
    /// Seals the channel — no further records — and returns the
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
    pub unsafe fn seal(mem: M) -> Result<Self, ProtocolError> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid for the reader's lifetime — and so for every use of
        // the views, which are stored in and dropped with the reader.
        let mapped = unsafe { MappedLayout::new(mem.as_raw_slice()) };
        let slot_count;
        let mut frames = 0;
        let complete;
        {
            // The seal boundary (rule 1): claims at or before this
            // snapshot are inside it, later ones land in slots this pass
            // never visits. The count is clamped to the table capacity, so
            // a counter inflated by failed claims (or by a foreign
            // scribble) degrades to a full-table sweep, not an error.
            let claims = mapped.claims().load(Ordering::Relaxed);
            slot_count = usize::try_from(claims & !CLOSED).unwrap_or(usize::MAX).min(SLOTS);
            // The same load carries the completeness verdict: a gate set
            // before this boundary is a failed claim's loss report — or an
            // earlier seal, and a re-seal cannot vouch for records
            // refused since then (rule 1).
            complete = claims & CLOSED == 0;

            // Gate further claims, so stragglers stop claiming (and
            // materializing pages) forever. Cheap: the creator pre-faulted
            // this page where first touches are expensive. Claims racing
            // between the snapshot and this gate are dropped soundly (see
            // the module docs above).
            mapped.claims().fetch_or(CLOSED, Ordering::Relaxed);

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
                    ABORTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) else {
                    // The receiver won the race: the unfinished slot is now
                    // aborted and stays ignored.
                    continue;
                };
                if bits == ABORTED {
                    // Aborted by an earlier seal over the same region;
                    // still ignored.
                    continue;
                }
                // Any other terminal value must be a committed descriptor
                // with a valid span; a foreign scribble fails the decode.
                if decode(mapped.payloads.len(), bits).is_none() {
                    return Err(ProtocolError { slot_index });
                }
                frames += 1;
            }
        }

        Ok(Self { mem, mapped, slot_count, frames, complete })
    }

    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> Iter<'_, SLOTS> {
        self.into_iter()
    }

    /// Whether every record a writer published made it in.
    ///
    /// False when a claim failed before the seal — the region
    /// was out of space, or a frame exceeded the frame limit: its record
    /// was lost, and the frames under-report what writers went on to do.
    /// Consumers that need completeness must reject them.
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
            // The slot is terminal (`seal` froze it), so this plain load
            // reads the same value the freeze pass saw; the transfer that
            // carried the reader to this thread carried the freeze pass's
            // `Acquire` payload visibility with it (rule 3).
            let bits = slot.load(Ordering::Relaxed);
            // `None` is an aborted slot: nothing was published. Corrupt
            // values cannot appear — `seal` already failed the channel on
            // them — so every decoded span is one `seal` validated.
            let Some(span) = decode(self.mapped.payloads.len(), bits) else {
                continue;
            };
            self.remaining -= 1;
            // SAFETY: `seal` validated the span against the payload
            // region, and a committed span is immutable for the mapping's
            // lifetime (see the module docs above); the reader borrowed
            // for `'a` keeps the mapping alive and mapped.
            return Some(unsafe {
                slice::from_raw_parts(
                    self.mapped.payloads.cast::<u8>().cast_const().add(span.offset),
                    span.len,
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

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::{super::writer::committed, *};

    #[test]
    fn payload_span_validates_bounds() {
        let region = 1024;
        // Any byte range inside the region, at any offset.
        assert!(PayloadSpan::validate(region, 0, 8).is_some());
        assert!(PayloadSpan::validate(region, 3, 5).is_some());
        // A span ending exactly at the region end.
        assert!(PayloadSpan::validate(region, region - 5, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(region, 0, 0).is_none());
        // The span may not cross the end of the region.
        assert!(PayloadSpan::validate(region, region - 8, 9).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(region, 0, layout::MAX_PAYLOAD_LEN + 1).is_none());
    }

    #[test]
    fn decode_roundtrips_committed_values() {
        let region = 1024;
        let span = decode(region, committed(0, 5)).unwrap();
        assert!(span.offset == 0 && span.len == 5);

        // The extremes of the descriptor fields on the largest mapping: the
        // 31-bit length limit, and a span ending exactly at the region end.
        let region = layout::MAX_PAYLOAD_REGION_LEN;
        let span = decode(region, committed(0, layout::MAX_PAYLOAD_LEN)).unwrap();
        assert!(span.offset == 0 && span.len == layout::MAX_PAYLOAD_LEN);
        let span = decode(region, committed(region - 8, 8)).unwrap();
        assert!(span.offset == region - 8 && span.len == 8);
    }

    #[test]
    fn decode_rejects_values_no_writer_commits() {
        let region = 1024;
        // The non-committed slot states.
        assert!(decode(region, layout::UNFINISHED).is_none());
        assert!(decode(region, ABORTED).is_none());
        // The aborted bit combined with other bits: the length field then
        // exceeds the frame limit.
        assert!(decode(region, ABORTED | 1).is_none());
        assert!(decode(region, ABORTED | (1 << 62)).is_none());
        // A zero length with a nonzero offset.
        assert!(decode(region, 42).is_none());
    }
}

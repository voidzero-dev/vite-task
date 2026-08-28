//! The reader side: seal the channel, then iterate the committed frames.
//!
//! Neither step walks the table or waits for a writer, and no payload byte
//! is copied. The reader keeps the mapping alive and hands out each
//! committed span on demand. Those borrows hold because nothing writes to
//! a committed span again (committing uses up the writer's frame) and it
//! never overlaps what a live writer may touch.
//!
//! A span's offset and length come from the writer that reserved them, and
//! nothing here re-checks them. That rests on the promise made at attach:
//! only this protocol touches the region. A process that scribbles on it
//! some other way breaks the promise, and this code does not defend
//! against that.

use std::{
    fmt,
    iter::FusedIterator,
    ptr::NonNull,
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{CLOSED, MappedLayout, SlotState, to_usize},
};

/// Why a channel could not be sealed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SealError {
    /// The mapping cannot hold the protocol at all (see
    /// [`MappedLayout::new`]).
    #[error("the shared-memory region cannot host the channel")]
    UnsupportedRegion,
    /// The channel was already closed when the seal ran. A claim had
    /// failed, or someone sealed earlier and this seal cannot say what was
    /// refused since. Either way the frames are not all of them, so the
    /// reader hands back none.
    #[error("the shared-memory channel was closed before it was sealed")]
    Closed,
}

/// A reader over the committed frames of a sealed channel, serving them
/// straight out of the mapping. It holds no buffer of its own, so sealing
/// allocates nothing; dropping it releases the mapping.
pub struct ShmReader<M> {
    /// Where the payload area is, for turning descriptors into spans.
    payload_start: NonNull<u8>,
    /// The slots the seal admitted, in claim order.
    table: NonNull<[AtomicU64]>,
    /// Owns the region the pointers point into. Declared after them:
    /// fields drop in order, and the borrower must go first.
    _mem: M,
}

// SAFETY: the reader only loads counters and descriptors atomically, and
// reads committed spans that nothing writes to any more; the stored
// pointers point into the mapped memory, which is owned separately and
// does not move, not into the reader itself.
unsafe impl<M: Send> Send for ShmReader<M> {}
// SAFETY: see the `Send` impl.
unsafe impl<M: Sync> Sync for ShmReader<M> {}

impl<M: AsRawSlice> ShmReader<M> {
    /// Seals the channel against further records and returns the reader of
    /// its committed frames.
    ///
    /// A reader exists only for a channel that kept everything. If a claim
    /// had already failed, or someone sealed earlier, there is no complete
    /// set of records and this fails instead.
    ///
    /// One atomic operation fixes how far iteration goes and shuts the
    /// gate, so it neither waits for a writer nor walks the table. A claim
    /// taken before that point but committed after it shows up if the
    /// store lands before the read reaches its slot; one taken after it
    /// lands in a slot iteration never reaches.
    ///
    /// # Safety
    ///
    /// Same contract as [`ShmWriter::new`](super::ShmWriter::new):
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the reader's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    /// - `slots` must be the count the region was created with.
    ///
    /// # Errors
    ///
    /// [`SealError`]: the mapping cannot hold the protocol, or the channel
    /// was already closed before this call.
    pub unsafe fn seal(mem: M, slots: usize) -> Result<Self, SealError> {
        // SAFETY: forwarded from this function's contract, which keeps the
        // region valid for as long as the reader lives, and so for every
        // use of the pointers, which are stored in the reader and dropped
        // with it.
        let Some(mapped) = (unsafe { MappedLayout::new(mem.as_raw_slice(), slots) }) else {
            return Err(SealError::UnsupportedRegion);
        };

        // Draw the boundary and shut the gate in one step (rule 1): this
        // returns the claim count at the instant no later claim can
        // succeed. Replacing the count rather than keeping it is fine,
        // since nothing reads it from here on: a later claim fails on the
        // gate before it uses its slot index, and a later seal only tests
        // the bit.
        let claims = mapped.claims().swap(CLOSED, Ordering::Relaxed);
        // The same value says whether anything was lost. A gate already
        // set means a failed claim, which rule 1 puts before this
        // boundary, or an earlier seal this one cannot account for.
        if claims & CLOSED != 0 {
            return Err(SealError::Closed);
        }
        // The count runs past the table whenever writers attempt more
        // claims than there are slots: one bumps the counter, finds its
        // index out of range, and only then sets the gate, so a seal
        // landing in that window reads the higher count with the gate
        // still clear. Such a count names slots that do not exist, so the
        // fall-back reads the whole table.
        //
        // Not an error, either: that writer performs the operation it
        // failed to record only after this boundary, and if it died first
        // it never performed one at all.
        let slots = mapped.table();
        let admitted = slots.get(..to_usize(claims)).unwrap_or(slots);

        // The admitted slots, kept as a raw pointer so the reader needs
        // no lifetime.
        // SAFETY of every later read through it: it points into the
        // mapping this reader owns, and a slot is an `AtomicU64`, so a
        // writer storing a descriptor never invalidates it.
        let table = NonNull::from_ref(admitted);
        Ok(Self { payload_start: mapped.payload_start, table, _mem: mem })
    }

    /// Iterates over the committed frames in claim order.
    ///
    /// Reads the descriptor table as it goes, so a writer that was still
    /// filling a frame when the channel was sealed may appear in a later
    /// call and not an earlier one. Everything it yields is a whole frame
    /// whose writer finished it.
    pub const fn iter(&self) -> Iter<'_> {
        Iter {
            payload_start: self.payload_start,
            // SAFETY: the slots live in the mapping this reader owns, and
            // each one is an `AtomicU64`, so writers storing descriptors
            // through their own pointers never invalidate this borrow. It
            // lasts no longer than `&self`, and so no longer than the
            // mapping.
            table: unsafe { self.table.as_ref() },
        }
    }
}

impl<M> fmt::Debug for ShmReader<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShmReader").field("slots", &self.table.len()).finish_non_exhaustive()
    }
}

/// Iterator over a [`ShmReader`]'s committed frames, in claim order.
pub struct Iter<'a> {
    /// Where the payload area is, for turning descriptors into spans.
    payload_start: NonNull<u8>,
    /// The admitted slots this iterator has not reached yet.
    table: &'a [AtomicU64],
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((slot, rest)) = self.table.split_first() {
            self.table = rest;
            // Rule 3: `Acquire`, so a descriptor this load sees brings
            // its payload bytes with it.
            let slot_value = slot.load(Ordering::Acquire);
            // An unfinished slot published nothing.
            let SlotState::Committed { offset, len } = SlotState::decode(slot_value) else {
                continue;
            };
            // SAFETY: a committed descriptor names the span its writer
            // reserved inside the payload area, and the attach contract
            // says nothing but this protocol writes the region, so these
            // are the bits a writer put here. Nothing writes to a
            // committed span any more, and the reader borrowed for `'a`
            // keeps the mapping alive.
            return Some(unsafe {
                slice::from_raw_parts(
                    self.payload_start.add(to_usize(offset)).as_ptr().cast_const(),
                    to_usize(len.get()),
                )
            });
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // One frame per remaining slot at most; how many are committed is
        // only known by reading them.
        (0, Some(self.table.len()))
    }
}

impl FusedIterator for Iter<'_> {}

impl<'a, M: AsRawSlice> IntoIterator for &'a ShmReader<M> {
    type IntoIter = Iter<'a>;
    type Item = &'a [u8];

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

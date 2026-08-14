//! The only module that touches shared-memory bytes.
//!
//! [`SharedState`] wraps the raw mapping and exposes the protocol's atomic
//! operations. Every unsafe pointer derivation lives here, justified by the
//! construction contract of [`SharedState::borrow`]; no other module reads or
//! writes the mapping directly.
//!
//! # Memory-ordering contract
//!
//! Three synchronization rules cover the whole protocol:
//!
//! 1. **Claim versus close** — both are read-modify-writes of the allocator
//!    word, so its modification order alone decides whether a claim is
//!    admitted before the close snapshot. A claim publishes no payload data,
//!    so `Relaxed` suffices ([`SharedState::try_claim`]). The `INCOMPLETE`
//!    flag rides the same word: a writer sets it (`Relaxed` RMW) before
//!    performing the operation whose record was lost, and the receiver
//!    re-reads the word after freezing ([`SharedState::reload`]); a flag the
//!    receiver's reload misses therefore belongs to an operation performed
//!    after close, outside the tracking boundary.
//! 2. **Writer commit** — the slot compare-and-swap uses `Release`
//!    ([`SharedState::commit`]): every payload write happens-before the
//!    committed descriptor becomes visible.
//! 3. **Receiver observation** — the freeze compare-and-swap uses `Acquire`
//!    on failure ([`SharedState::freeze`]): observing a committed descriptor
//!    also makes the payload writes it published visible, so the copy in
//!    [`SharedState::read_payload`] reads settled bytes.
//!
//! Payload reads use `Relaxed` atomic loads rather than plain loads: committed
//! payloads are immutable under the protocol, but the mapping is writable in
//! every traced process, so a buggy foreign process can scribble concurrently.
//! Atomic loads keep such races from being undefined behavior in this
//! process — each load returns *some* value, and torn garbage surfaces as a
//! frame-decoding error instead of a crash.

use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    alloc_word::{self, AllocWord, Reservation, ReserveError},
    layout::{self, PayloadSpan},
    slot,
};

/// A borrowed view of the shared mapping with protocol-level operations.
#[derive(Clone, Copy)]
pub(super) struct SharedState<'m> {
    base: *mut u8,
    len: usize,
    _mapping: PhantomData<&'m ()>,
}

impl<'m> SharedState<'m> {
    /// Borrows a shared mapping.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes for the lifetime `'m` and
    ///   its address must be stable.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    /// - Other processes may access the region concurrently, but only through
    ///   this protocol.
    ///
    /// # Panics
    ///
    /// Panics when the mapping cannot host the protocol at all: base not
    /// word-aligned, or length outside
    /// `[layout::HEADER_LEN, layout::MAX_MAPPING_LEN]`. These indicate a
    /// broken caller, not runtime data.
    pub(super) unsafe fn borrow(mem: *mut [u8]) -> Self {
        let base = mem.cast::<u8>();
        assert!(base.addr().is_multiple_of(align_of::<AtomicU64>()));
        assert!(mem.len() <= layout::MAX_MAPPING_LEN);
        // Ignore any sub-word tail so payload spans growing from the end
        // stay word-aligned.
        let len = layout::usable_len(mem.len());
        assert!(len >= layout::HEADER_LEN);
        Self { base, len, _mapping: PhantomData }
    }

    pub(super) const fn mapping_len(self) -> usize {
        self.len
    }

    /// The allocator word at the start of the header.
    const fn word(self) -> &'m AtomicU64 {
        // SAFETY: `borrow` checked that the mapping is word-aligned and at
        // least `HEADER_LEN` bytes, so the first 8 bytes are a valid, aligned
        // `AtomicU64` for `'m`.
        unsafe { AtomicU64::from_ptr(self.base.cast()) }
    }

    /// The descriptor slot at `index`.
    ///
    /// # Panics
    ///
    /// Panics when the slot lies outside the mapping. Callers only pass
    /// indices below an admitted (writer) or validated (receiver) slot
    /// count, so the assertion documents an invariant rather than guarding
    /// runtime data.
    fn slot_atomic(self, index: usize) -> &'m AtomicU64 {
        assert!(layout::table_end(index + 1) <= self.len);
        // SAFETY: the assertion keeps the slot inside the mapping, and the
        // table consists of word-aligned 8-byte slots after the aligned
        // header.
        unsafe { AtomicU64::from_ptr(self.base.add(layout::table_end(index)).cast()) }
    }

    /// Reads the allocator word without synchronization (rule 1: claims and
    /// flags need no payload visibility).
    pub(super) fn load_word(self) -> AllocWord {
        AllocWord::from_bits(self.word().load(Ordering::Relaxed))
    }

    /// Forces the page holding the allocator word to be materialized by the
    /// operating system before a latency-sensitive path writes it.
    ///
    /// A compare-exchange of zero with zero: on an untouched page it performs
    /// a real write — allocating the first block of a sparse backing file,
    /// which can cost milliseconds on journalling filesystems — without
    /// changing protocol state. If a claim got there first, the page is
    /// already backed and the failed exchange changes nothing. (An `or` of
    /// zero would not do: the compiler may lower it to a plain load, which
    /// maps the shared zero page without allocating anything.)
    pub(super) fn pre_fault(self) {
        let _ = self.word().compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Atomically reserves one descriptor slot and one payload span.
    pub(super) fn try_claim(self, payload_len: usize) -> Result<Reservation, ReserveError> {
        let word = self.word();
        let mut current = word.load(Ordering::Relaxed);
        loop {
            let reservation = AllocWord::from_bits(current).reserve(payload_len, self.len)?;
            // Rule 1: `Relaxed` — admission is decided by the modification
            // order of the allocator word alone.
            match word.compare_exchange_weak(
                current,
                reservation.new_word.bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(reservation),
                Err(actual) => current = actual,
            }
        }
    }

    /// Records that a frame this process may still act on was lost.
    ///
    /// Must be called before the operation whose record was lost is
    /// performed (rule 1).
    pub(super) fn flag_incomplete(self) {
        self.word().fetch_or(alloc_word::INCOMPLETE, Ordering::Relaxed);
    }

    /// Closes the channel and snapshots the admitted slot count.
    pub(super) fn close(self) -> AllocWord {
        AllocWord::from_bits(self.word().fetch_or(alloc_word::CLOSED, Ordering::AcqRel))
    }

    /// Re-reads the allocator word; used after the freeze pass to observe
    /// `INCOMPLETE` flags set while closing (rule 1).
    pub(super) fn reload(self) -> AllocWord {
        AllocWord::from_bits(self.word().load(Ordering::Acquire))
    }

    /// Publishes a committed descriptor into an unfinished slot.
    ///
    /// Returns false when the receiver aborted the slot first; the payload is
    /// then permanently unreachable and the writer must not touch it again
    /// either way.
    pub(super) fn commit(self, slot_index: usize, descriptor: u64) -> bool {
        // Rule 2: `Release` orders every payload write before the descriptor.
        self.slot_atomic(slot_index)
            .compare_exchange(slot::UNFINISHED, descriptor, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }

    /// Freezes one slot during close and returns its terminal value: `ABORTED`
    /// when the receiver won the race, the committed descriptor otherwise.
    pub(super) fn freeze(self, slot_index: usize) -> u64 {
        // Rule 3: `Acquire` on failure makes a committed payload visible.
        match self.slot_atomic(slot_index).compare_exchange(
            slot::UNFINISHED,
            slot::ABORTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => slot::ABORTED,
            Err(terminal) => terminal,
        }
    }

    /// Pointer to a reserved payload span. The caller owns the span's
    /// exclusivity argument.
    pub(super) fn payload_ptr(self, offset: usize) -> *mut u8 {
        debug_assert!(offset <= self.len);
        // SAFETY: callers pass offsets of admitted reservations, which
        // `layout::fits` keeps inside the mapping.
        unsafe { self.base.add(offset) }
    }

    /// Appends the payload bytes of a validated span to `out`.
    ///
    /// Reads the span's word-aligned reservation with `Relaxed` atomic loads
    /// (see the module docs) and appends exactly `span.len` bytes.
    pub(super) fn read_payload(self, span: PayloadSpan, out: &mut Vec<u8>) {
        let keep = out.len() + span.len;
        for word_index in 0..span.reserved_len() / layout::SLOT_LEN {
            let offset = span.offset + word_index * layout::SLOT_LEN;
            // SAFETY: `PayloadSpan::validate` checked that the word-aligned
            // reservation `[span.offset, span.offset + span.reserved_len())`
            // lies inside the mapping, and `span.offset` is word-aligned.
            let word = unsafe { AtomicU64::from_ptr(self.base.add(offset).cast()) };
            out.extend_from_slice(&word.load(Ordering::Relaxed).to_ne_bytes());
        }
        // Drop the sub-word padding bytes of the final word.
        out.truncate(keep);
    }
}

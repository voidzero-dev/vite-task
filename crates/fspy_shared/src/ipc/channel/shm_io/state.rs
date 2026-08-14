//! The only module that touches shared-memory bytes.
//!
//! [`SharedState`] wraps the raw mapping and exposes the protocol's atomic
//! operations. Every unsafe pointer derivation lives here, justified by the
//! construction contract of [`SharedState::borrow`]; no other module reads or
//! writes the mapping directly.
//!
//! # Shared words
//!
//! The header holds three independent monotonic words (offsets in
//! [`layout`]):
//!
//! - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//!   claims ever attempted. Claiming is one wait-free `fetch_add`; the
//!   returned old value carries the claim's slot index, the gate, and — by
//!   comparison against the fixed table capacity — the capacity verdict.
//!   Failed claims still count, so the counter can overshoot the capacity;
//!   readers clamp instead of trusting it.
//! - the **payload counter**: payload bytes ever reserved, bumped by another
//!   wait-free `fetch_add`. Also overshoots on failure. Never read by the
//!   receiver — committed descriptors are self-describing.
//! - the **incomplete flag**: nonzero once a live writer lost a record.
//!
//! # Memory-ordering contract
//!
//! Three synchronization rules cover the whole protocol:
//!
//! 1. **Claim versus close** — the receiver's close boundary is a plain
//!    snapshot load of the claim counter: claims ordered at or before the
//!    value it reads (in the counter's modification order) are in the
//!    snapshot; later ones receive slot indices the receiver never visits.
//!    Claims publish no payload data, so `Relaxed` suffices throughout.
//!    The CLOSED gate is set *after* collection (off the latency-sensitive
//!    path): it only stops stragglers from working and allocating pages
//!    forever; any claim admitted between the snapshot and the gate lands
//!    beyond the snapshot and is never observed. The incomplete flag rides
//!    the same rule: a writer sets it (`Relaxed` RMW) before performing the
//!    operation whose record was lost, and the receiver re-reads it after
//!    freezing; a flag the receiver misses therefore belongs to an operation
//!    performed after the boundary.
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
    layout::{self, PayloadSpan},
    slot,
};

/// The CLOSED gate bit of the claim counter. The low 63 bits count claims,
/// so no realistic claim volume can carry into the gate.
const CLOSED: u64 = 1 << 63;

/// Why a claim was not admitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ReserveError {
    /// The receiver has closed the channel.
    Closed,
    /// The frame is oversized, or its region is out of capacity.
    Capacity,
}

/// A successful reservation of one descriptor slot and one payload span.
#[derive(Debug)]
pub(super) struct Reservation {
    /// Index of the reserved descriptor slot.
    pub(super) slot_index: usize,
    /// Byte offset of the reserved payload span. Word-aligned.
    pub(super) payload_offset: usize,
}

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
        let len = mem.len();
        assert!(base.addr().is_multiple_of(align_of::<AtomicU64>()));
        assert!((layout::HEADER_LEN..=layout::MAX_MAPPING_LEN).contains(&len));
        Self { base, len, _mapping: PhantomData }
    }

    pub(super) const fn mapping_len(self) -> usize {
        self.len
    }

    /// Number of descriptor slots the fixed table holds.
    pub(super) const fn max_slots(self) -> usize {
        layout::max_slots(self.len)
    }

    /// A header word. `offset` must be one of the `layout` header offsets.
    fn header_word(self, offset: usize) -> &'m AtomicU64 {
        debug_assert!(offset < layout::HEADER_LEN);
        // SAFETY: `borrow` checked that the mapping is word-aligned and at
        // least `HEADER_LEN` bytes, so every word-aligned header offset is a
        // valid, aligned `AtomicU64` for `'m`.
        unsafe { AtomicU64::from_ptr(self.base.add(offset).cast()) }
    }

    /// The descriptor slot at `index`.
    ///
    /// # Panics
    ///
    /// Panics when the slot lies outside the fixed table. Callers only pass
    /// indices below an admitted (writer) or clamped (receiver) slot count,
    /// so the assertion documents an invariant rather than guarding runtime
    /// data.
    fn slot_atomic(self, index: usize) -> &'m AtomicU64 {
        assert!(index < self.max_slots());
        // SAFETY: the assertion keeps the slot inside the fixed table, which
        // consists of word-aligned 8-byte slots after the aligned header.
        unsafe {
            AtomicU64::from_ptr(self.base.add(layout::HEADER_LEN + index * layout::SLOT_LEN).cast())
        }
    }

    /// Whether the CLOSED gate has been set.
    pub(super) fn is_closed(self) -> bool {
        self.header_word(layout::SLOT_COUNTER_OFFSET).load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Atomically reserves one descriptor slot and one payload span.
    ///
    /// Wait-free: two `fetch_add`s, no retry loop (rule 1).
    pub(super) fn try_claim(self, payload_len: usize) -> Result<Reservation, ReserveError> {
        if payload_len > layout::MAX_PAYLOAD_LEN {
            return Err(ReserveError::Capacity);
        }
        let reserved_len = layout::reserved_payload_len(payload_len);

        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot. A failed reservation stays counted — overshoot is harmless
        // because the counter is not what locates payloads (descriptors are)
        // and a `u64` cannot realistically wrap.
        let payload_start = self
            .header_word(layout::PAYLOAD_COUNTER_OFFSET)
            .fetch_add(reserved_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(reserved_len as u64);
        if payload_end.is_none_or(|end| end > layout::payload_region_len(self.len) as u64) {
            return Err(ReserveError::Capacity);
        }

        let claims = self.header_word(layout::SLOT_COUNTER_OFFSET).fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            return Err(ReserveError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= self.max_slots() {
            return Err(ReserveError::Capacity);
        }

        // The capacity check bounded `payload_start` by the region length,
        // which fits `usize`.
        let payload_start = usize::try_from(payload_start).expect("bounded by the payload region");
        Ok(Reservation {
            slot_index,
            // In bounds: `payload_start + reserved_len` fits the region, and
            // the region ends within the mapping (`layout`).
            payload_offset: layout::payload_base(self.len) + payload_start,
        })
    }

    /// Records that a frame this process may still act on was lost.
    ///
    /// Must be called before the operation whose record was lost is
    /// performed (rule 1).
    pub(super) fn flag_incomplete(self) {
        self.header_word(layout::INCOMPLETE_OFFSET).fetch_or(1, Ordering::Relaxed);
    }

    /// Whether any live writer lost a record. Read after the freeze pass
    /// (rule 1).
    pub(super) fn is_incomplete(self) -> bool {
        self.header_word(layout::INCOMPLETE_OFFSET).load(Ordering::Relaxed) != 0
    }

    /// Snapshots the number of admitted claims: the receiver's close
    /// boundary (rule 1). Clamped to the table capacity because failed
    /// claims overshoot the counter.
    pub(super) fn snapshot_claims(self) -> usize {
        let claims = self.header_word(layout::SLOT_COUNTER_OFFSET).load(Ordering::Relaxed);
        usize::try_from(claims & !CLOSED).unwrap_or(usize::MAX).min(self.max_slots())
    }

    /// Forces the page holding the header counters to be materialized by
    /// the operating system before anyone touches it on a latency-sensitive
    /// path.
    ///
    /// A compare-exchange of zero with zero on the claim counter: on an
    /// untouched region it performs a real write — allocating the first
    /// block of a sparse backing file, which can cost milliseconds on
    /// journalling filesystems — without changing protocol state. If a
    /// claim got there first, the page is already backed and the failed
    /// exchange changes nothing. (An `or` of zero would not do: the
    /// compiler may lower it to a plain load, which materializes only a
    /// hole page without allocating the block.)
    ///
    /// Only Linux channels use this: elsewhere the first touch is cheap.
    #[cfg(target_os = "linux")]
    pub(super) fn pre_fault(self) {
        let _ = self.header_word(layout::SLOT_COUNTER_OFFSET).compare_exchange(
            0,
            0,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );

        // Also materialize the page where the first payloads land, through
        // the protocol-owned warm word so no writer's payload bytes are ever
        // raced. Skipped when a tiny region has no room for it.
        let warm_offset = layout::payload_warm_offset(self.len);
        if warm_offset + layout::SLOT_LEN <= self.len {
            // SAFETY: in bounds (just checked) and word-aligned (the table
            // length is word-rounded after the aligned header).
            let warm_word = unsafe { AtomicU64::from_ptr(self.base.add(warm_offset).cast()) };
            let _ = warm_word.compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    /// Sets the CLOSED gate so stragglers stop claiming.
    ///
    /// Not part of the close boundary (rule 1): run it after collection,
    /// off the latency-sensitive path — on an otherwise-untouched region
    /// this write materializes the counter page, which can cost
    /// milliseconds of first-block allocation on journalling filesystems.
    pub(super) fn close_claims(self) {
        self.header_word(layout::SLOT_COUNTER_OFFSET).fetch_or(CLOSED, Ordering::Relaxed);
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
        // `layout` keeps inside the mapping.
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
            // lies inside the payload region, and `span.offset` is
            // word-aligned.
            let word = unsafe { AtomicU64::from_ptr(self.base.add(offset).cast()) };
            out.extend_from_slice(&word.load(Ordering::Relaxed).to_ne_bytes());
        }
        // Drop the sub-word padding bytes of the final word.
        out.truncate(keep);
    }
}

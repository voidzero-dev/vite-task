//! The only module that touches shared-memory bytes.
//!
//! [`SharedState`] wraps the raw mapping and exposes the protocol's atomic
//! operations. The header and descriptor table are one `repr(C)` struct,
//! [`Region`], borrowed from the mapping base with a single unsafe cast in
//! [`SharedState::borrow`]; every access after that is a plain field access
//! or an index into the slot array. Only the untyped payload area — which
//! must stay outside the [`Region`] referent so writers' exclusive `&mut`
//! payload spans never alias it — is still reached through raw pointers.
//!
//! # Shared atomics
//!
//! The header holds three independent monotonic `AtomicU64`s:
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
//!    The CLOSED gate only stops stragglers from claiming (and allocating
//!    pages) forever; any claim admitted between the snapshot and the gate
//!    lands beyond the snapshot and is never observed. The incomplete flag
//!    rides the same rule: a writer sets it (`Relaxed` RMW) before
//!    performing the operation whose record was lost, and the receiver
//!    re-reads it after freezing; a flag the receiver misses therefore
//!    belongs to an operation performed after the boundary.
//! 2. **Writer commit** — the slot compare-and-swap uses `Release`
//!    ([`SharedState::commit`]): every payload write happens-before the
//!    committed descriptor becomes visible.
//! 3. **Receiver observation** — the freeze compare-and-swap uses `Acquire`
//!    on failure ([`SharedState::freeze`]): observing a committed descriptor
//!    also makes the payload writes it published visible, so the borrows the
//!    receiver later hands out (see `reader`) read settled bytes.

use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{layout, slot};

/// The CLOSED gate bit of the claim counter. The low 63 bits count claims,
/// so no realistic claim volume can carry into the gate.
const CLOSED: u64 = 1 << 63;

/// The region header: three protocol atomics, padded so the descriptor
/// table starts off their cache line and there is room for future header
/// fields, which must start zeroed.
#[repr(C)]
struct Header {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    claims: AtomicU64,
    /// Nonzero once a live writer lost a record.
    incomplete: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    payload_reserved: AtomicU64,
    _reserved: [u64; 5],
}

const _: () = assert!(size_of::<Header>() == layout::HEADER_LEN);
const _: () = assert!(align_of::<Header>() == align_of::<AtomicU64>());

/// The compile-time-laid-out prefix of the region: the header and the
/// descriptor table. The payload area follows it in the mapping but is
/// deliberately not a field — writers hold exclusive `&mut` borrows into
/// it, which must not alias the shared `&Region` borrow.
#[repr(C)]
struct Region<const SLOTS: usize> {
    header: Header,
    slots: [AtomicU64; SLOTS],
}

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
    /// Byte offset of the reserved payload span. `u64`-aligned.
    pub(super) payload_offset: usize,
}

/// A borrowed view of the shared mapping with protocol-level operations.
#[derive(Clone, Copy)]
pub(super) struct SharedState<'m, const SLOTS: usize> {
    region: &'m Region<SLOTS>,
    base: *mut u8,
    len: usize,
    _mapping: PhantomData<&'m ()>,
}

impl<const SLOTS: usize> SharedState<'_, SLOTS> {
    /// Byte offset where the payload region starts: right after the
    /// descriptor table. `u64`-aligned by construction.
    const PAYLOAD_BASE: usize = {
        assert!(size_of::<Region<SLOTS>>() == layout::payload_base_for_slots(SLOTS));
        size_of::<Region<SLOTS>>()
    };

    /// Borrows a shared mapping.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes for the lifetime `'m` and
    ///   its address must be stable.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since,
    ///   instantiated with the same `SLOTS` by every process.
    ///
    /// # Panics
    ///
    /// Panics when the mapping cannot host the protocol at all: base not
    /// `u64`-aligned, too small for the header and table, or larger than
    /// [`layout::MAX_MAPPING_LEN`]. These indicate a broken caller, not
    /// runtime data.
    pub(super) unsafe fn borrow(mem: *mut [u8]) -> Self {
        let base = mem.cast::<u8>();
        let len = mem.len();
        assert!(base.addr().is_multiple_of(align_of::<Region<SLOTS>>()));
        assert!((Self::PAYLOAD_BASE..=layout::MAX_MAPPING_LEN).contains(&len));
        // SAFETY: the region prefix `[base, base + size_of::<Region>())` is
        // in bounds and aligned (both asserted above) and consists entirely
        // of atomics zero-initialized at creation, so a shared borrow for
        // `'m` is valid even while other threads and processes access the
        // same memory — they do so through these same atomics.
        let region = unsafe { &*base.cast::<Region<SLOTS>>() };
        Self { region, base, len, _mapping: PhantomData }
    }

    pub(super) const fn mapping_len(self) -> usize {
        self.len
    }

    /// Byte offset where the payload region starts.
    #[expect(
        clippy::unused_self,
        reason = "reads an associated const; instance syntax keeps call sites uniform"
    )]
    pub(super) const fn payload_base(self) -> usize {
        Self::PAYLOAD_BASE
    }

    /// Whether the CLOSED gate has been set.
    pub(super) fn is_closed(self) -> bool {
        self.region.header.claims.load(Ordering::Relaxed) & CLOSED != 0
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
        let payload_start =
            self.region.header.payload_reserved.fetch_add(reserved_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(reserved_len as u64);
        let payload_region_len = layout::payload_region_len(self.len, Self::PAYLOAD_BASE);
        if payload_end.is_none_or(|end| end > payload_region_len as u64) {
            return Err(ReserveError::Capacity);
        }

        let claims = self.region.header.claims.fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            return Err(ReserveError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= SLOTS {
            return Err(ReserveError::Capacity);
        }

        Ok(Reservation {
            slot_index,
            // In bounds: `payload_start + reserved_len` fits the payload
            // region, which ends within the mapping (`layout`).
            payload_offset: Self::PAYLOAD_BASE
                + usize::try_from(payload_start).expect("bounded by the payload region"),
        })
    }

    /// Records that a frame this process may still act on was lost.
    ///
    /// Must be called before the operation whose record was lost is
    /// performed (rule 1).
    pub(super) fn flag_incomplete(self) {
        self.region.header.incomplete.fetch_or(1, Ordering::Relaxed);
    }

    /// Whether any live writer lost a record. Read after the freeze pass
    /// (rule 1).
    pub(super) fn is_incomplete(self) -> bool {
        self.region.header.incomplete.load(Ordering::Relaxed) != 0
    }

    /// Snapshots the number of admitted claims: the receiver's close
    /// boundary (rule 1). Clamped to the table capacity because failed
    /// claims overshoot the counter.
    pub(super) fn snapshot_claims(self) -> usize {
        let claims = self.region.header.claims.load(Ordering::Relaxed);
        usize::try_from(claims & !CLOSED).unwrap_or(usize::MAX).min(SLOTS)
    }

    /// Sets the CLOSED gate so stragglers stop claiming.
    pub(super) fn close_claims(self) {
        self.region.header.claims.fetch_or(CLOSED, Ordering::Relaxed);
    }

    /// Forces the page backing the header (and the table's first slots) to
    /// be materialized by the operating system before anyone touches it on
    /// a latency-sensitive path.
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
        let _ =
            self.region.header.claims.compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Publishes a committed descriptor into an unfinished slot.
    ///
    /// Returns false when the receiver aborted the slot first; the payload is
    /// then permanently unreachable and the writer must not touch it again
    /// either way.
    ///
    /// # Panics
    ///
    /// Panics when `slot_index` lies outside the table; callers only pass
    /// indices of admitted reservations.
    pub(super) fn commit(self, slot_index: usize, descriptor: u64) -> bool {
        // Rule 2: `Release` orders every payload write before the descriptor.
        self.region.slots[slot_index]
            .compare_exchange(slot::UNFINISHED, descriptor, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }

    /// Freezes one slot during close and returns its terminal value: `ABORTED`
    /// when the receiver won the race, the committed descriptor otherwise.
    ///
    /// # Panics
    ///
    /// Panics when `slot_index` lies outside the table; the receiver only
    /// passes indices below its clamped snapshot.
    pub(super) fn freeze(self, slot_index: usize) -> u64 {
        // Rule 3: `Acquire` on failure makes a committed payload visible.
        match self.region.slots[slot_index].compare_exchange(
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
}

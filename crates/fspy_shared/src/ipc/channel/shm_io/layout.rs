//! Everything the writer and the reader sides share.
//!
//! The region is divided into three fixed areas:
//!
//! ```text
//! | header | descriptor table | payloads (grow up) |
//! ```
//!
//! This module holds the region's shape — the sizing rule that turns a
//! mapping length into table and payload bounds, payload rounding, and
//! payload-span validation — the header type, the descriptor-slot codec
//! both sides encode and decode, and [`MappedLayout`]: the shape bound to
//! one concrete mapping, built once when an endpoint attaches. The sides
//! themselves live in [`super::writer`] and [`super::reader`].
//!
//! Overflow safety follows from one bound enforced at construction time:
//! the mapping length never exceeds [`MAX_MAPPING_LEN`], so all offsets fit
//! the 32-bit descriptor fields and all sums fit `usize` on the 64-bit
//! targets the parent module asserts.

use std::{ptr::NonNull, sync::atomic::AtomicU64};

/// The CLOSED gate bit of the claim counter, set when the receiver seals
/// the channel and by any writer whose claim failed — the loss report
/// that also condemns the channel (rule 1 below).
/// The low 63 bits count claims, so no realistic claim volume can carry
/// into the gate.
pub(super) const CLOSED: u64 = 1 << 63;

/// The region header: two protocol counters, padded so the descriptor
/// table starts off their cache line and there is room for future header
/// fields, which must start zeroed.
#[repr(C)]
pub(super) struct Header {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    pub(super) claims: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    pub(super) payload_reserved: AtomicU64,
    _reserved: [u64; 6],
}

// One cache line: shrink `_reserved` when adding a field. The alignment is
// what lets a `u64`-aligned mapping base be cast to `&Header`.
const _: () = assert!(size_of::<Header>() == 64);
const _: () = assert!(align_of::<Header>() == align_of::<AtomicU64>());

/// Byte size of the region header, taken from [`Header`] itself.
pub(super) const HEADER_LEN: usize = size_of::<Header>();

/// Byte size of one descriptor slot.
pub(super) const SLOT_LEN: usize = size_of::<u64>();

/// Maximum payload size of a single frame.
///
/// Committed lengths are stored in the 31-bit length field of a descriptor,
/// which caps them at `i32::MAX`.
pub(super) const MAX_PAYLOAD_LEN: usize = i32::MAX as usize;

/// Maximum supported mapping size.
///
/// Descriptors store payload offsets in 32 bits, so the payload region
/// must fit `u32` arithmetic; capping the whole mapping at 4 GiB keeps
/// every offset and sum inside it.
pub(super) const MAX_MAPPING_LEN: usize = 1 << 32;

/// Minimum supported mapping size — a deliberate cutoff, not a derived
/// one: regions under a KiB are not worth a channel. It keeps every
/// supported region's shape regular (at least 15 slots and 840 payload
/// bytes), so the sizing rules below need no small-region special cases.
pub(super) const MIN_MAPPING_LEN: usize = 1024;

/// Whether `len` is a supported mapping length: a multiple of the slot
/// size between [`MIN_MAPPING_LEN`] and [`MAX_MAPPING_LEN`]. Everything
/// below assumes a supported length.
pub(super) const fn is_supported(len: usize) -> bool {
    len.is_multiple_of(SLOT_LEN) && len >= MIN_MAPPING_LEN && len <= MAX_MAPPING_LEN
}

/// The descriptor-table length of a supported `mapping_len`-byte region.
///
/// Both endpoints derive the layout from the mapping length alone, so the
/// region is self-describing: no side channel has to agree on a table
/// size. An eighth of the space beyond the header goes to descriptors —
/// generous slack for typical record shapes, one 8-byte descriptor per
/// payload of a few hundred bytes — and the region is sparse, so an
/// oversized table costs address space, not memory.
pub(super) const fn table_slots(mapping_len: usize) -> usize {
    (mapping_len - HEADER_LEN) / (8 * SLOT_LEN)
}

/// Rounds a payload length up to a multiple of `size_of::<u64>()`.
///
/// Payload reservations are whole `u64`s, so every payload offset stays
/// `u64`-aligned — an invariant [`PayloadSpan::validate`] uses to reject
/// descriptors no correct writer produces. The sub-`u64` padding stays
/// inside the frame's own reservation.
pub(super) const fn reserved_payload_len(payload_len: usize) -> usize {
    payload_len.next_multiple_of(SLOT_LEN)
}

/// Byte size of the payload region of a supported `mapping_len`-byte
/// mapping: everything after the table. A multiple of `size_of::<u64>()`
/// by construction — supported lengths, the header, and the table all
/// are — so a reservation of whole `u64`s inside it never reaches past
/// `mapping_len`.
pub(super) const fn payload_region_len(mapping_len: usize) -> usize {
    mapping_len - HEADER_LEN - table_slots(mapping_len) * SLOT_LEN
}

// --- The descriptor slot codec ---------------------------------------------
//
// One slot is a 64-bit value that publishes a frame:
//
// ```text
// bit 63     bits 32..=62               bits 0..=31
// ABORTED    payload length (31)        payload offset (32)
// ```
//
// | Value                 | State                                           |
// | --------------------- | ----------------------------------------------- |
// | `0`                   | Unfinished: slot reserved, nothing published    |
// | `1 << 63`             | Aborted: the receiver froze the unfinished slot |
// | nonzero, bit 63 clear | Committed: offset and length of the payload     |
//
// Committed lengths are nonzero (a zero-length frame is never claimed), so a
// committed value is always nonzero and the three states are disjoint. Once
// a slot is committed or aborted, nothing ever changes it again.
//
// Offsets are measured from the start of the payload area, so no
// descriptor can even name the header or the table.
//
// This defines the format both sides must agree on; encoding lives with
// the writer ([`super::writer`]) and decoding with the reader
// ([`super::reader`]).

pub(super) const UNFINISHED: u64 = 0;
pub(super) const LEN_SHIFT: u32 = 32;
pub(super) const OFFSET_MAX: u64 = u32::MAX as u64;

// --- The mapped layout and the ordering contract ---------------------------
// `MappedLayout::new` builds three typed views of the region — the
// `repr(C)` `Header`, the descriptor table as a slice of atomics sized by
// the mapping length, and the untyped payload area as a raw slice — once,
// when an endpoint attaches; the endpoint stores them beside the mapping
// they point into. Every access after that is a plain field access or a
// bounds-checked index. The payload area stays raw because writers hold
// exclusive `&mut` borrows into it, which must not alias any shared
// reference.
//
// # Shared atomics
//
// The header holds two independent monotonic `AtomicU64` counters:
//
// - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//   claims ever attempted. Claiming is one wait-free `fetch_add`; the
//   returned old value carries the claim's slot index, the gate, and — by
//   comparison against the fixed table capacity — the capacity verdict.
//   The gate is set by the receiver's seal and by every failed claim:
//   one bit is both the loss report completeness derives from and the
//   valve that stops writers spending work on a channel whose result the
//   receiver must already reject.
// - the **payload counter**: payload bytes ever reserved, bumped by another
//   wait-free `fetch_add`.
//
// Failed claims leave the counters bumped; that is harmless, because the
// receiver clamps instead of trusting the counts, and committed
// descriptors carry their own offset and length, so the counters never
// locate data. The payload counter can even wrap on a long-condemned
// channel — still harmless: wrapping requires prior failures, failures
// set the gate, and the gate refuses every claim before a span is built.
//
// # Memory-ordering contract
//
// Three synchronization rules cover the whole protocol:
//
// 1. **Claim versus seal** — the receiver's seal boundary is a plain
//    snapshot load of the claim counter: claims ordered at or before the
//    value it reads (in the counter's modification order) are in the
//    snapshot; later ones receive slot indices the receiver never visits.
//    Claims publish no payload data, so `Relaxed` suffices throughout.
//    The CLOSED gate is not itself the boundary — it stops stragglers
//    from claiming (and allocating pages) forever; any claim admitted
//    between the snapshot and the gate lands beyond the snapshot and is
//    never observed. Completeness rides the same modification order: a
//    failed claim sets the gate as its loss report before the writer
//    performs the operation whose record was lost — so the snapshot
//    either sees the bit, or the loss belongs to an operation performed
//    after the boundary. A writer that skipped because it saw the bit is
//    covered the same way: the bit that made it skip either reaches the
//    snapshot or postdates the boundary. A writer that dies before
//    setting the bit never performed its operation, so nothing was
//    actually lost.
// 2. **Writer commit** — the slot compare-and-swap in `FrameMut::finish`
//    uses `Release`: every payload write happens-before the committed
//    descriptor becomes visible.
// 3. **Receiver observation** — the freeze compare-and-swap in
//    `ShmReader::seal` uses `Acquire` on failure: observing a committed
//    descriptor also makes the payload writes it published visible, so the
//    borrows `ShmReader` later hands out read settled bytes.

/// The typed views of the region: the header, the descriptor table
/// sized from the mapping length, and the raw payload area. Built once
/// when an endpoint attaches and stored in it; the views stay valid
/// because they point into the mapping's stable target, not into the
/// endpoint value.
#[derive(Clone, Copy)]
pub(super) struct MappedLayout {
    header: NonNull<Header>,
    table: NonNull<[AtomicU64]>,
    pub(super) payloads: *mut [u8],
}

impl MappedLayout {
    /// Builds the typed views of a shared mapping.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes, and its address stable,
    ///   for as long as the returned views (and any copy of them) are
    ///   used.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    ///
    /// # Panics
    ///
    /// Panics when the mapping cannot host the protocol at all: base not
    /// `u64`-aligned, or a length that is not supported ([`is_supported`]).
    /// These indicate a broken caller, not runtime data; senders guard
    /// untrusted mappings with [`super::is_supported_region_len`] first.
    #[expect(clippy::cast_ptr_alignment, reason = "the base is asserted `u64`-aligned below")]
    pub(super) unsafe fn new(mem: *mut [u8]) -> Self {
        let base = mem.cast::<u8>();
        let len = mem.len();
        assert!(!base.is_null());
        assert!(base.addr().is_multiple_of(align_of::<Header>()));
        assert!(is_supported(len));
        // SAFETY: the base is non-null (asserted), and the header and the
        // table lie inside the mapping — the header by the asserts above,
        // the table by `max_slots` — at `u64`-aligned offsets. The
        // payload area keeps the rest of the mapping as a raw slice;
        // `layout` bounds every span carved from it.
        unsafe {
            Self {
                header: NonNull::new_unchecked(base.cast::<Header>()),
                table: NonNull::slice_from_raw_parts(
                    NonNull::new_unchecked(base.add(HEADER_LEN).cast::<AtomicU64>()),
                    table_slots(len),
                ),
                payloads: std::ptr::slice_from_raw_parts_mut(
                    base.add(HEADER_LEN + table_slots(len) * SLOT_LEN),
                    payload_region_len(len),
                ),
            }
        }
    }

    /// The protocol header.
    pub(super) const fn header(&self) -> &Header {
        // SAFETY: `new`'s contract keeps the target valid while any view
        // is used, and the header consists of atomics, so the shared
        // borrow is valid even while other threads and processes access
        // the same memory through them.
        unsafe { self.header.as_ref() }
    }

    /// The descriptor table.
    pub(super) const fn table(&self) -> &[AtomicU64] {
        // SAFETY: as for `header`.
        unsafe { self.table.as_ref() }
    }
}

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::*;

    #[test]
    fn reserved_payload_len_rounds_up_to_u64s() {
        assert!(reserved_payload_len(1) == 8);
        assert!(reserved_payload_len(7) == 8);
        assert!(reserved_payload_len(8) == 8);
        assert!(reserved_payload_len(9) == 16);
        assert!(reserved_payload_len(MAX_PAYLOAD_LEN) == MAX_PAYLOAD_LEN + 1);
    }

    #[test]
    fn table_gets_an_eighth_of_the_region() {
        assert!(table_slots(1 << 20) == 16383);
        // The 4 GiB production mapping: ~67M slots.
        assert!(table_slots(MAX_MAPPING_LEN) == ((MAX_MAPPING_LEN - HEADER_LEN) / 8) / 8);
    }

    #[test]
    fn unsupported_lengths_are_refused() {
        assert!(is_supported(MIN_MAPPING_LEN));
        assert!(is_supported(MAX_MAPPING_LEN));
        // Too small, even as a multiple of 8.
        assert!(!is_supported(1000));
        assert!(!is_supported(0));
        // Not a multiple of 8.
        assert!(!is_supported(1025));
        // Too large.
        assert!(!is_supported(MAX_MAPPING_LEN + 8));
    }

    #[test]
    fn payload_region_fills_the_rest() {
        assert!(table_slots(1024) == 15);
        assert!(payload_region_len(1024) == 840);
        // The three areas cover a supported region exactly.
        assert!(
            HEADER_LEN
                + table_slots(MAX_MAPPING_LEN) * SLOT_LEN
                + payload_region_len(MAX_MAPPING_LEN)
                == MAX_MAPPING_LEN
        );
    }
}

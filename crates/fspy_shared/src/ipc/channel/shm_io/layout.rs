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

/// The CLOSED gate bit of the claim counter, set by the receiver when it
/// closes the channel and by any writer whose claim failed — the loss
/// report that also condemns the channel (the parent module's rule 1).
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
/// Payload offsets are stored in 32 bits, so every byte offset into the
/// mapping must fit `u32` arithmetic.
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
pub(super) const fn max_slots(mapping_len: usize) -> usize {
    (mapping_len - HEADER_LEN) / (8 * SLOT_LEN)
}

/// Byte offset where the payload region starts: right after the table.
/// `u64`-aligned.
pub(super) const fn payload_base(mapping_len: usize) -> usize {
    HEADER_LEN + max_slots(mapping_len) * SLOT_LEN
}

/// Rounds a payload length up to a multiple of `size_of::<u64>()`.
///
/// Payload reservations are whole `u64`s — combined with the `u64`-aligned
/// region base they grow from, every payload offset stays `u64`-aligned, an
/// invariant [`PayloadSpan::validate`] uses to reject descriptors no correct
/// writer produces. The sub-`u64` padding stays inside the frame's own
/// reservation.
pub(super) const fn reserved_payload_len(payload_len: usize) -> usize {
    payload_len.next_multiple_of(SLOT_LEN)
}

/// Byte size of the payload region of a supported `mapping_len`-byte
/// mapping: everything after the table. A multiple of `size_of::<u64>()`
/// by construction — supported lengths, the header, and the table all
/// are — so a reservation of whole `u64`s inside it never reaches past
/// `mapping_len`.
pub(super) const fn payload_region_len(mapping_len: usize) -> usize {
    mapping_len - payload_base(mapping_len)
}

/// A validated payload byte range: the witness that offset arithmetic on this
/// span cannot leave the payload region.
///
/// Constructing a `PayloadSpan` through [`PayloadSpan::validate`] is the
/// single validation point for descriptor metadata read back from shared
/// memory; code holding a span may rely on its bounds without re-checking.
#[derive(Clone, Copy, Debug)]
pub(super) struct PayloadSpan {
    /// Byte offset of the payload from the start of the mapping.
    /// Always `u64`-aligned.
    pub(super) offset: usize,
    /// Exact (unpadded) byte length of the payload.
    pub(super) len: usize,
}

impl PayloadSpan {
    /// Validates a committed descriptor's payload range against a payload
    /// region starting at `payload_base` and `payload_region_len` bytes
    /// long. Returns `None` if the range could not have been produced by a
    /// correct writer.
    pub(super) const fn validate(
        payload_base: usize,
        payload_region_len: usize,
        offset: usize,
        len: usize,
    ) -> Option<Self> {
        if len == 0 || len > MAX_PAYLOAD_LEN {
            return None;
        }
        // Writers reserve whole-`u64` spans from the `u64`-aligned region
        // base, so a valid offset is `u64`-aligned and its padded length
        // stays inside the region.
        if !offset.is_multiple_of(SLOT_LEN) {
            return None;
        }
        // `offset` and `len` come from 32-bit descriptor fields, so these
        // sums cannot overflow `usize`.
        if offset < payload_base
            || offset + reserved_payload_len(len) > payload_base + payload_region_len
        {
            return None;
        }
        Some(Self { offset, len })
    }
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
// The receiver never classifies invalid values: [`decode`] accepts exactly
// the committed descriptors a correct writer can produce for the mapping,
// and refuses everything else alike — an unfinished `0` decodes a zero
// length, and any value carrying the aborted bit decodes a length beyond
// [`MAX_PAYLOAD_LEN`], so both fail [`PayloadSpan::validate`].

pub(super) const UNFINISHED: u64 = 0;
pub(super) const ABORTED: u64 = 1 << 63;
const LEN_SHIFT: u32 = 32;
const OFFSET_MAX: u64 = u32::MAX as u64;

/// Encodes a committed descriptor.
///
/// The caller guarantees `payload_len` is `1..=MAX_PAYLOAD_LEN` and
/// `payload_offset` fits 32 bits; both hold for any admitted reservation.
pub(super) fn committed(payload_offset: usize, payload_len: usize) -> u64 {
    debug_assert!(payload_len > 0 && payload_len <= MAX_PAYLOAD_LEN);
    debug_assert!(payload_offset as u64 <= OFFSET_MAX);
    ((payload_len as u64) << LEN_SHIFT) | payload_offset as u64
}

/// Decodes a slot value read back from shared memory as a committed
/// descriptor. Returns `None` for any value that is not one a correct
/// writer could have committed for the payload region described by
/// `payload_base` and `payload_region_len`.
pub(super) const fn decode(
    payload_base: usize,
    payload_region_len: usize,
    bits: u64,
) -> Option<PayloadSpan> {
    PayloadSpan::validate(
        payload_base,
        payload_region_len,
        (bits & OFFSET_MAX) as usize,
        (bits >> LEN_SHIFT) as usize,
    )
}

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
//   The gate is set by the receiver at close and by every failed claim:
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
// 1. **Claim versus close** — the receiver's close boundary is a plain
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
//    `ShmReader::close` uses `Acquire` on failure: observing a committed
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
                    max_slots(len),
                ),
                payloads: std::ptr::slice_from_raw_parts_mut(
                    base.add(payload_base(len)),
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

    /// Base address of the region: the header sits at offset zero.
    pub(super) const fn base(&self) -> *const u8 {
        self.header.as_ptr().cast()
    }

    /// Byte offset where the payload region starts: right after the table.
    pub(super) const fn payload_base(&self) -> usize {
        HEADER_LEN + self.table().len() * SLOT_LEN
    }

    /// Decodes a slot value from this mapping's descriptor table. See
    /// [`decode`].
    pub(super) const fn decode(&self, bits: u64) -> Option<PayloadSpan> {
        decode(self.payload_base(), self.payloads.len(), bits)
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
    fn max_slots_gives_an_eighth_to_the_table() {
        assert!(max_slots(1 << 20) == 16383);
        // The 4 GiB production mapping: ~67M slots.
        assert!(max_slots(MAX_MAPPING_LEN) == ((MAX_MAPPING_LEN - HEADER_LEN) / 8) / 8);
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
        assert!(max_slots(1024) == 15);
        assert!(payload_base(1024) == 184);
        assert!(payload_region_len(1024) == 840);
        // The three areas cover a supported region exactly.
        assert!(
            payload_base(MAX_MAPPING_LEN) + payload_region_len(MAX_MAPPING_LEN) == MAX_MAPPING_LEN
        );
    }

    #[test]
    fn payload_span_validates_bounds() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        // A `u64`-aligned span at the region start.
        assert!(PayloadSpan::validate(base, region, base, 8).is_some());
        // Exact end of the region, with padding inside it.
        assert!(PayloadSpan::validate(base, region, base + region - 8, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(base, region, base, 0).is_none());
        // Padded length may not cross the end of the region.
        assert!(PayloadSpan::validate(base, region, base + region - 8, 9).is_none());
        // Payloads may not reach into the descriptor table or header.
        assert!(PayloadSpan::validate(base, region, base - 8, 8).is_none());
        assert!(PayloadSpan::validate(base, region, 0, 8).is_none());
        // Unaligned offsets cannot come from a correct writer.
        assert!(PayloadSpan::validate(base, region, base + 4, 4).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(base, region, base, MAX_PAYLOAD_LEN + 1).is_none());
    }

    #[test]
    fn decode_roundtrips_committed_values() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        let span = decode(base, region, committed(base, 5)).unwrap();
        assert!(span.offset == base && span.len == 5);

        // The extremes of the descriptor fields on the largest mapping: the
        // 31-bit length limit, and a span ending exactly at the region end.
        let mapping_len = MAX_MAPPING_LEN;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        let span = decode(base, region, committed(base, MAX_PAYLOAD_LEN)).unwrap();
        assert!(span.offset == base && span.len == MAX_PAYLOAD_LEN);
        let last = base + region - 8;
        let span = decode(base, region, committed(last, 8)).unwrap();
        assert!(span.offset == last && span.len == 8);
    }

    #[test]
    fn decode_rejects_values_no_writer_commits() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        // The non-committed slot states.
        assert!(decode(base, region, UNFINISHED).is_none());
        assert!(decode(base, region, ABORTED).is_none());
        // The aborted bit combined with other bits: the length field then
        // exceeds `MAX_PAYLOAD_LEN`.
        assert!(decode(base, region, ABORTED | 1).is_none());
        assert!(decode(base, region, ABORTED | (1 << 62)).is_none());
        // A zero length with a nonzero offset.
        assert!(decode(base, region, 42).is_none());
    }
}

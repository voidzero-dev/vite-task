//! Pure geometry of the shared-memory region.
//!
//! The region is divided into three fixed areas:
//!
//! ```text
//! | header | descriptor table (SLOTS slots) | payloads (grow up) |
//! ```
//!
//! This module holds the region's shape, with no memory access: the
//! header struct, the sizing rule that turns a mapping length into table
//! and payload bounds, payload rounding, payload-span validation, and the
//! descriptor-slot codec. The operations on the region live in
//! [`super`].
//!
//! Overflow safety follows from one bound enforced at construction time:
//! the mapping length never exceeds [`MAX_MAPPING_LEN`], so all offsets fit
//! the 32-bit descriptor fields and all sums fit `usize` on the 64-bit
//! targets the parent module asserts.

use std::sync::atomic::AtomicU64;

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

/// The descriptor-table length of a `mapping_len`-byte region.
///
/// Both endpoints derive the layout from the mapping length alone, so the
/// region is self-describing: no side channel has to agree on a table
/// size. An eighth of the space for descriptors (floored at eight slots so
/// tiny regions stay usable) is generous slack for typical record shapes —
/// one 8-byte descriptor per payload of a few hundred bytes — and the
/// region is sparse, so an oversized table costs address space, not
/// memory.
pub(super) const fn max_slots(mapping_len: usize) -> usize {
    let available = mapping_len - HEADER_LEN;
    let len = available / 8;
    let len = if len < 8 * SLOT_LEN { 8 * SLOT_LEN } else { len };
    let len = if len > available { available } else { len };
    len / SLOT_LEN
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

/// Byte size of the payload region of a `mapping_len`-byte mapping. A
/// multiple of `size_of::<u64>()`, so a reservation of whole `u64`s inside
/// it never reaches past `mapping_len`.
pub(super) const fn payload_region_len(mapping_len: usize) -> usize {
    let base = payload_base(mapping_len);
    if base >= mapping_len {
        return 0;
    }
    let len = mapping_len - base;
    len - len % SLOT_LEN
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
    /// Validates a committed descriptor's payload range against the payload
    /// region of a `mapping_len`-byte mapping. Returns `None` if the range
    /// could not have been produced by a correct writer.
    pub(super) const fn validate(mapping_len: usize, offset: usize, len: usize) -> Option<Self> {
        if len == 0 || len > MAX_PAYLOAD_LEN {
            return None;
        }
        // Writers reserve whole-`u64` spans from the `u64`-aligned region
        // base, so a valid offset is `u64`-aligned and its padded length
        // stays inside the region.
        if !offset.is_multiple_of(SLOT_LEN) {
            return None;
        }
        let base = payload_base(mapping_len);
        // `offset` and `len` come from 32-bit descriptor fields, so these
        // sums cannot overflow `usize`.
        if offset < base
            || offset + reserved_payload_len(len) > base + payload_region_len(mapping_len)
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
/// writer could have committed for a `mapping_len`-byte mapping.
pub(super) const fn decode(mapping_len: usize, bits: u64) -> Option<PayloadSpan> {
    PayloadSpan::validate(mapping_len, (bits & OFFSET_MAX) as usize, (bits >> LEN_SHIFT) as usize)
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
    fn max_slots_floors_tiny_regions_at_eight_slots() {
        assert!(max_slots(1024) == 15);
        assert!(max_slots(256) == 8);
        // Not enough space for the floor: the table takes what exists and
        // payload capacity degrades to zero; claims fail gracefully.
        assert!(max_slots(100) == 4);
        assert!(max_slots(64) == 0);
    }

    #[test]
    fn payload_region_rounds_down_and_degrades_to_zero() {
        assert!(payload_base(1024) == 184);
        assert!(payload_region_len(1024) == 840);
        assert!(payload_region_len(1000) == 824);
        // The base at or past the mapping: no payload space at all.
        assert!(payload_region_len(100) == 0);
        assert!(payload_region_len(64) == 0);
    }

    #[test]
    fn payload_span_validates_bounds() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        // A `u64`-aligned span at the region start.
        assert!(PayloadSpan::validate(mapping_len, base, 8).is_some());
        // Exact end of the region, with padding inside it.
        assert!(PayloadSpan::validate(mapping_len, base + region - 8, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(mapping_len, base, 0).is_none());
        // Padded length may not cross the end of the region.
        assert!(PayloadSpan::validate(mapping_len, base + region - 8, 9).is_none());
        // Payloads may not reach into the descriptor table or header.
        assert!(PayloadSpan::validate(mapping_len, base - 8, 8).is_none());
        assert!(PayloadSpan::validate(mapping_len, 0, 8).is_none());
        // Unaligned offsets cannot come from a correct writer.
        assert!(PayloadSpan::validate(mapping_len, base + 4, 4).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(mapping_len, base, MAX_PAYLOAD_LEN + 1).is_none());
    }

    #[test]
    fn decode_roundtrips_committed_values() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let span = decode(mapping_len, committed(base, 5)).unwrap();
        assert!(span.offset == base && span.len == 5);

        // The extremes of the descriptor fields on the largest mapping: the
        // 31-bit length limit, and a span ending exactly at the region end.
        let mapping_len = MAX_MAPPING_LEN;
        let base = payload_base(mapping_len);
        let span = decode(mapping_len, committed(base, MAX_PAYLOAD_LEN)).unwrap();
        assert!(span.offset == base && span.len == MAX_PAYLOAD_LEN);
        let last = base + payload_region_len(mapping_len) - 8;
        let span = decode(mapping_len, committed(last, 8)).unwrap();
        assert!(span.offset == last && span.len == 8);
    }

    #[test]
    fn decode_rejects_values_no_writer_commits() {
        let mapping_len = 1024;
        // The non-committed slot states.
        assert!(decode(mapping_len, UNFINISHED).is_none());
        assert!(decode(mapping_len, ABORTED).is_none());
        // The aborted bit combined with other bits: the length field then
        // exceeds `MAX_PAYLOAD_LEN`.
        assert!(decode(mapping_len, ABORTED | 1).is_none());
        assert!(decode(mapping_len, ABORTED | (1 << 62)).is_none());
        // A zero length with a nonzero offset.
        assert!(decode(mapping_len, 42).is_none());
    }
}

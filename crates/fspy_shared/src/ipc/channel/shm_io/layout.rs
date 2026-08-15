//! Pure geometry of the shared-memory region.
//!
//! The region is divided into three fixed areas:
//!
//! ```text
//! | header | descriptor table (SLOTS slots) | payloads (grow up) |
//! ```
//!
//! The header and table have compile-time layout and live in `state`'s
//! `repr(C)` region struct; this module holds the plain-integer arithmetic
//! around them: the table-sizing rule for a given capacity, payload
//! rounding, and payload-span validation. No pointers, no atomics.
//!
//! Overflow safety follows from one bound enforced at construction time:
//! the mapping length never exceeds [`MAX_MAPPING_LEN`], so all offsets fit
//! the 32-bit descriptor fields and all sums fit `usize` on the 64-bit
//! targets the parent module asserts.

/// Byte size of the region header. Kept in this type-free module for the
/// sizing arithmetic; `state` asserts it equals `size_of::<Header>()`.
pub(super) const HEADER_LEN: usize = 64;

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

/// The descriptor-table length for a region of `capacity` bytes: the
/// `SLOTS` parameter a creator should instantiate the protocol with.
///
/// An eighth of the space for descriptors (floored at eight slots so tiny
/// regions stay usable) is generous slack for typical record shapes — one
/// 8-byte descriptor per payload of a few hundred bytes — and the region is
/// sparse, so an oversized table costs address space, not memory.
#[must_use]
pub const fn slots_for_capacity(capacity: usize) -> usize {
    let available = capacity - HEADER_LEN;
    let len = available / 8;
    let len = if len < 8 * SLOT_LEN { 8 * SLOT_LEN } else { len };
    let len = if len > available { available } else { len };
    len / SLOT_LEN
}

/// The byte capacity a creator should size the region to for a table of
/// `slots` descriptors.
///
/// Covers the header, the table, and the payload region the sizing rule
/// implies (seven bytes of payload space per table byte — the exact
/// inverse of [`slots_for_capacity`] for eight or more slots).
#[must_use]
pub const fn capacity_for_slots(slots: usize) -> usize {
    HEADER_LEN + slots * SLOT_LEN * 8
}

/// Byte offset where the payload region starts: right after the table.
/// `state` asserts it equals `size_of::<Region<SLOTS>>()`.
pub(super) const fn payload_base_for_slots(slots: usize) -> usize {
    HEADER_LEN + slots * SLOT_LEN
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

/// Byte size of the payload region of a `mapping_len`-byte mapping whose
/// payloads start at `payload_base`. A multiple of `size_of::<u64>()`, so a
/// reservation of whole `u64`s inside it never reaches past `mapping_len`.
pub(super) const fn payload_region_len(mapping_len: usize, payload_base: usize) -> usize {
    if payload_base >= mapping_len {
        return 0;
    }
    let len = mapping_len - payload_base;
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
    /// region `[payload_base, payload_base + payload_region_len)` of a
    /// `mapping_len`-byte mapping. Returns `None` if the range could not
    /// have been produced by a correct writer.
    pub(super) const fn validate(
        mapping_len: usize,
        payload_base: usize,
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
            || offset + reserved_payload_len(len)
                > payload_base + payload_region_len(mapping_len, payload_base)
        {
            return None;
        }
        Some(Self { offset, len })
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
    fn slots_for_capacity_gives_an_eighth_to_the_table() {
        assert!(slots_for_capacity(1 << 20) == 16383);
        // The 4 GiB production mapping: ~67M slots.
        assert!(slots_for_capacity(MAX_MAPPING_LEN) == ((MAX_MAPPING_LEN - HEADER_LEN) / 8) / 8);
    }

    #[test]
    fn slots_for_capacity_floors_tiny_regions_at_eight_slots() {
        assert!(slots_for_capacity(1024) == 15);
        assert!(slots_for_capacity(256) == 8);
        // Not enough space for the floor: the table takes what exists and
        // payload capacity degrades to zero; claims fail gracefully.
        assert!(slots_for_capacity(100) == 4);
        assert!(slots_for_capacity(64) == 0);
    }

    #[test]
    fn capacity_and_slots_round_trip() {
        for slots in [8, 15, 1023, slots_for_capacity(MAX_MAPPING_LEN)] {
            assert!(slots_for_capacity(capacity_for_slots(slots)) == slots);
        }
        // The production 4 GiB capacity round-trips exactly.
        assert!(capacity_for_slots(slots_for_capacity(MAX_MAPPING_LEN)) == MAX_MAPPING_LEN);
    }

    #[test]
    fn payload_region_rounds_down_and_degrades_to_zero() {
        assert!(payload_region_len(1024, 184) == 840);
        assert!(payload_region_len(1000, 184) == 816);
        // The base at or past the mapping: no payload space at all.
        assert!(payload_region_len(100, 104) == 0);
        assert!(payload_region_len(100, 100) == 0);
    }

    #[test]
    fn payload_span_validates_bounds() {
        let mapping_len = 1024;
        let base = 184;
        let region = payload_region_len(mapping_len, base);
        // A `u64`-aligned span at the region start.
        assert!(PayloadSpan::validate(mapping_len, base, base, 8).is_some());
        // Exact end of the region, with padding inside it.
        assert!(PayloadSpan::validate(mapping_len, base, base + region - 8, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(mapping_len, base, base, 0).is_none());
        // Padded length may not cross the end of the region.
        assert!(PayloadSpan::validate(mapping_len, base, base + region - 8, 9).is_none());
        // Payloads may not reach into the descriptor table or header.
        assert!(PayloadSpan::validate(mapping_len, base, base - 8, 8).is_none());
        assert!(PayloadSpan::validate(mapping_len, base, 0, 8).is_none());
        // Unaligned offsets cannot come from a correct writer.
        assert!(PayloadSpan::validate(mapping_len, base, base + 4, 4).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(mapping_len, base, base, MAX_PAYLOAD_LEN + 1).is_none());
    }
}

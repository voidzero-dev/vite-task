//! Pure geometry of the shared-memory region.
//!
//! The mapping is divided into three areas:
//!
//! ```text
//! low addresses                                              high addresses
//! | header | descriptor table (grows up) | free | payloads (grow down) |
//! ```
//!
//! Everything in this module is arithmetic on plain integers — no atomics,
//! no pointers, no shared state. Overflow safety follows from two bounds
//! enforced at construction time and re-validated on every value read back
//! from shared memory: the mapping length never exceeds [`MAX_MAPPING_LEN`]
//! (so all offsets fit in the 32-bit descriptor fields) and all region
//! arithmetic is performed in `usize` on 64-bit targets (asserted in the
//! parent module), where sums of 32-bit-bounded quantities cannot overflow.

/// Byte size of the region header.
///
/// Only the first 8 bytes (the allocator word) are used; the rest keeps the
/// descriptor table off the allocator word's cache line and leaves room for
/// future header fields, which must start zeroed.
pub(super) const HEADER_LEN: usize = 64;

/// Byte size of one descriptor slot.
pub(super) const SLOT_LEN: usize = size_of::<u64>();

/// Maximum payload size of a single frame.
///
/// Committed lengths are stored in the 31-bit length field of a descriptor,
/// which caps them at `i32::MAX` — the same frame-size limit as the previous
/// inline-header format.
pub(super) const MAX_PAYLOAD_LEN: usize = i32::MAX as usize;

/// Maximum supported mapping size.
///
/// Payload offsets and the reserved-payload counter are stored in 32 bits, so
/// every byte offset into the mapping must fit in `u32` arithmetic; a mapping
/// of exactly 4 GiB works because no payload can start at the very end.
pub(super) const MAX_MAPPING_LEN: usize = 1 << 32;

/// Rounds a payload length up to a multiple of the word size.
///
/// Payload reservations are word-aligned — combined with the word-aligned
/// [`usable_len`] they grow down from, this keeps every payload offset
/// word-aligned so the receiver can copy payloads with aligned 64-bit atomic
/// loads, and the sub-word padding stays inside the frame's own reservation.
pub(super) const fn reserved_payload_len(payload_len: usize) -> usize {
    payload_len.next_multiple_of(SLOT_LEN)
}

/// The protocol-usable prefix of a mapping: its length rounded down to word
/// alignment, so payload spans growing from the end stay word-aligned even
/// when the creator requested an odd capacity.
pub(super) const fn usable_len(mapping_len: usize) -> usize {
    mapping_len - mapping_len % SLOT_LEN
}

/// First byte offset after a descriptor table of `slot_count` slots.
pub(super) const fn table_end(slot_count: usize) -> usize {
    HEADER_LEN + slot_count * SLOT_LEN
}

/// Whether a mapping of `mapping_len` bytes can hold `slot_count` descriptors
/// and `payload_bytes` reserved payload bytes without the regions meeting.
///
/// Also the validity check for an allocator word read back from shared
/// memory: any word this function accepts yields in-bounds table and payload
/// regions.
pub(super) const fn fits(mapping_len: usize, slot_count: usize, payload_bytes: usize) -> bool {
    // Both operands are bounded by their allocator-word field widths (30 and
    // 32 bits), so the sum cannot overflow 64-bit `usize` arithmetic.
    table_end(slot_count) + payload_bytes <= mapping_len
}

/// A validated payload byte range: the witness that offset arithmetic on this
/// span cannot leave the mapping or touch the descriptor table.
///
/// Constructing a `PayloadSpan` through [`PayloadSpan::validate`] is the
/// single validation point for descriptor metadata read back from shared
/// memory; code holding a span may rely on its bounds without re-checking.
#[derive(Clone, Copy, Debug)]
pub(super) struct PayloadSpan {
    /// Byte offset of the payload from the start of the mapping.
    /// Always word-aligned.
    pub(super) offset: usize,
    /// Exact (unpadded) byte length of the payload.
    pub(super) len: usize,
}

impl PayloadSpan {
    /// Validates a committed descriptor's payload range against the final
    /// region layout. Returns `None` if the range could not have been
    /// produced by a correct writer.
    pub(super) const fn validate(
        mapping_len: usize,
        table_end: usize,
        offset: usize,
        len: usize,
    ) -> Option<Self> {
        if len == 0 || len > MAX_PAYLOAD_LEN {
            return None;
        }
        // Writers reserve word-aligned spans from the word-aligned mapping
        // end, so a valid offset is word-aligned and its padded length stays
        // inside the mapping.
        if !offset.is_multiple_of(SLOT_LEN) {
            return None;
        }
        // `offset` and `len` come from 32-bit descriptor fields, so this sum
        // cannot overflow `usize`.
        if offset < table_end || offset + reserved_payload_len(len) > mapping_len {
            return None;
        }
        Some(Self { offset, len })
    }

    /// The word-aligned length of the reservation containing this payload.
    pub(super) const fn reserved_len(self) -> usize {
        reserved_payload_len(self.len)
    }
}

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::*;

    #[test]
    fn reserved_payload_len_rounds_up_to_words() {
        assert!(reserved_payload_len(1) == 8);
        assert!(reserved_payload_len(7) == 8);
        assert!(reserved_payload_len(8) == 8);
        assert!(reserved_payload_len(9) == 16);
        assert!(reserved_payload_len(MAX_PAYLOAD_LEN) == MAX_PAYLOAD_LEN + 1);
    }

    #[test]
    fn usable_len_rounds_down_to_words() {
        assert!(usable_len(100) == 96);
        assert!(usable_len(96) == 96);
        assert!(usable_len(MAX_MAPPING_LEN) == MAX_MAPPING_LEN);
    }

    #[test]
    fn table_end_starts_after_header() {
        assert!(table_end(0) == HEADER_LEN);
        assert!(table_end(3) == HEADER_LEN + 24);
    }

    #[test]
    fn fits_detects_frontier_collision() {
        // 64-byte header + 1 slot + 8 payload bytes exactly fill 80 bytes.
        assert!(fits(80, 1, 8));
        assert!(!fits(80, 1, 16));
        assert!(!fits(80, 2, 8));
        assert!(!fits(72, 1, 8));
    }

    #[test]
    fn fits_handles_the_4_gib_mapping() {
        let max_payload = MAX_MAPPING_LEN - HEADER_LEN - SLOT_LEN;
        assert!(fits(MAX_MAPPING_LEN, 1, max_payload));
        assert!(!fits(MAX_MAPPING_LEN, 1, max_payload + 8));
        // The largest admissible slot count leaves no payload space.
        let max_slots = (MAX_MAPPING_LEN - HEADER_LEN) / SLOT_LEN;
        assert!(fits(MAX_MAPPING_LEN, max_slots, 0));
        assert!(!fits(MAX_MAPPING_LEN, max_slots + 1, 0));
    }

    #[test]
    fn payload_span_validates_bounds() {
        let table_end = table_end(2);
        // A word-aligned span inside the payload region.
        assert!(PayloadSpan::validate(1024, table_end, 1016, 8).is_some());
        // Exact end of the mapping.
        assert!(PayloadSpan::validate(1024, table_end, 1016, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(1024, table_end, 512, 0).is_none());
        // Padded length may not cross the end of the mapping.
        assert!(PayloadSpan::validate(1024, table_end, 1020, 8).is_none());
        assert!(PayloadSpan::validate(1024, table_end, 1016, 9).is_none());
        // Payloads may not reach into the descriptor table.
        assert!(PayloadSpan::validate(1024, table_end, table_end - 8, 8).is_none());
        // Unaligned offsets cannot come from a correct writer.
        assert!(PayloadSpan::validate(1024, table_end, 1017, 7).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(1024, table_end, 512, MAX_PAYLOAD_LEN + 1).is_none());
    }
}

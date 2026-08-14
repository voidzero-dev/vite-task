//! Pure geometry of the shared-memory region.
//!
//! The mapping is divided into three fixed areas:
//!
//! ```text
//! | header | descriptor table (fixed capacity) | payloads (grow up) |
//! ```
//!
//! The table gets an eighth of the space after the header (with a small
//! floor so tiny regions stay usable). An eighth is generous slack for
//! typical record shapes — one 8-byte descriptor per payload of a few
//! hundred bytes — and the region is sparse, so an oversized table costs
//! address space, not memory.
//!
//! Everything in this module is arithmetic on plain integers — no atomics,
//! no pointers, no shared state. Overflow safety follows from one bound
//! enforced at construction time: the mapping length never exceeds
//! [`MAX_MAPPING_LEN`], so all offsets fit the 32-bit descriptor fields and
//! all sums fit `usize` on the 64-bit targets the parent module asserts.

/// Byte size of the region header: the slot counter, the incomplete flag,
/// and the payload counter, padded so the descriptor table starts off the
/// counters' cache line and there is room for future header fields, which
/// must start zeroed.
pub(super) const HEADER_LEN: usize = 64;

/// Byte offset of the claim counter (one `u64`: the CLOSED gate bit and the
/// number of claims attempted).
pub(super) const SLOT_COUNTER_OFFSET: usize = 0;

/// Byte offset of the incomplete flag word (nonzero once a live writer lost
/// a record).
pub(super) const INCOMPLETE_OFFSET: usize = 8;

/// Byte offset of the payload counter (one `u64`: payload bytes reserved,
/// including by failed claims — monotonic, never read by the receiver).
pub(super) const PAYLOAD_COUNTER_OFFSET: usize = 16;

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
/// Payload offsets are stored in 32 bits, so every byte offset into the
/// mapping must fit `u32` arithmetic.
pub(super) const MAX_MAPPING_LEN: usize = 1 << 32;

/// Rounds a payload length up to a multiple of the word size.
///
/// Payload reservations are word-aligned — combined with the word-aligned
/// region base they grow from, this keeps every payload offset word-aligned
/// so the receiver can copy payloads with aligned 64-bit atomic loads, and
/// the sub-word padding stays inside the frame's own reservation.
pub(super) const fn reserved_payload_len(payload_len: usize) -> usize {
    payload_len.next_multiple_of(SLOT_LEN)
}

/// Byte size of the descriptor table in a mapping of `mapping_len` bytes.
pub(super) const fn table_len(mapping_len: usize) -> usize {
    let available = mapping_len - HEADER_LEN;
    // An eighth for descriptors, floored at eight slots so small (test)
    // regions hold a few frames, and never more than the available space.
    let len = available / 8;
    let len = if len < 8 * SLOT_LEN { 8 * SLOT_LEN } else { len };
    let len = if len > available { available } else { len };
    len - len % SLOT_LEN
}

/// Number of descriptor slots in a mapping of `mapping_len` bytes.
pub(super) const fn max_slots(mapping_len: usize) -> usize {
    table_len(mapping_len) / SLOT_LEN
}

/// Byte offset of the payload warm word: one protocol-owned word between the
/// table and the payload data, written only by the creator's pre-fault (and
/// only ever with zero). Touching it materializes the page where the first
/// payloads land without racing any writer's payload bytes. Word-aligned.
pub(super) const fn payload_warm_offset(mapping_len: usize) -> usize {
    HEADER_LEN + table_len(mapping_len)
}

/// Byte offset where the payload region starts. Word-aligned.
///
/// May exceed a tiny mapping; [`payload_region_len`] is zero then and no
/// payload is ever placed.
pub(super) const fn payload_base(mapping_len: usize) -> usize {
    payload_warm_offset(mapping_len) + SLOT_LEN
}

/// Byte size of the payload region. A multiple of the word size, so a
/// word-aligned reservation inside it never reaches past `mapping_len`.
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
    /// Always word-aligned.
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
        // Writers reserve word-aligned spans from the word-aligned region
        // base, so a valid offset is word-aligned and its padded length stays
        // inside the region.
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
    fn partition_is_aligned_and_disjoint() {
        for mapping_len in [64, 100, 128, 1024, 4096, 1 << 20, MAX_MAPPING_LEN] {
            let table = table_len(mapping_len);
            let base = payload_base(mapping_len);
            let region = payload_region_len(mapping_len);
            assert!(table % SLOT_LEN == 0);
            assert!(base % SLOT_LEN == 0);
            assert!(region % SLOT_LEN == 0);
            assert!(payload_warm_offset(mapping_len) == HEADER_LEN + table);
            assert!(base == HEADER_LEN + table + SLOT_LEN);
            assert!(region == 0 || base + region <= mapping_len);
        }
    }

    #[test]
    fn partition_gives_an_eighth_to_the_table() {
        assert!(table_len(1 << 20) == (1 << 20) / 8 - 8);
        assert!(max_slots(1 << 20) == 16383);
        // The 4 GiB production mapping: ~67M slots, ~3.4 GiB of payloads.
        assert!(max_slots(MAX_MAPPING_LEN) == ((MAX_MAPPING_LEN - HEADER_LEN) / 8) / 8);
    }

    #[test]
    fn tiny_regions_floor_the_table_at_eight_slots() {
        // Enough space: eight slots, remainder to payloads.
        assert!(max_slots(1024) == 15);
        assert!(max_slots(256) == 8);
        // Not enough space for the floor: the table takes what exists and
        // payload capacity degrades to zero; claims fail gracefully.
        assert!(table_len(100) == 32);
        assert!(payload_region_len(100) == 0);
        assert!(table_len(64) == 0);
        assert!(max_slots(64) == 0);
    }

    #[test]
    fn payload_span_validates_bounds() {
        let mapping_len = 1024;
        let base = payload_base(mapping_len);
        let region = payload_region_len(mapping_len);
        // A word-aligned span at the region start.
        assert!(PayloadSpan::validate(mapping_len, base, 8).is_some());
        // Exact end of the region, with padding inside it.
        assert!(PayloadSpan::validate(mapping_len, base + region - 8, 5).is_some());
        // Zero length is never committed.
        assert!(PayloadSpan::validate(mapping_len, base, 0).is_none());
        // Padded length may not cross the end of the region.
        assert!(PayloadSpan::validate(mapping_len, base + region - 8, 9).is_none());
        // Payloads may not reach into the descriptor table.
        assert!(PayloadSpan::validate(mapping_len, base - 8, 8).is_none());
        assert!(PayloadSpan::validate(mapping_len, HEADER_LEN, 8).is_none());
        // Unaligned offsets cannot come from a correct writer.
        assert!(PayloadSpan::validate(mapping_len, base + 4, 4).is_none());
        // Oversized lengths are rejected before any arithmetic.
        assert!(PayloadSpan::validate(mapping_len, base, MAX_PAYLOAD_LEN + 1).is_none());
    }
}

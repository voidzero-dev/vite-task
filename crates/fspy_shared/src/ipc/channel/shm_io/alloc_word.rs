//! Encoding of the allocator word — the single 64-bit value that admits
//! claims, closes the channel, and records lost frames.
//!
//! ```text
//! bit 63    bit 62        bits 32..=61       bits 0..=31
//! CLOSED    INCOMPLETE    slot count (30)    reserved payload bytes (32)
//! ```
//!
//! - `CLOSED`: set once by the receiver; no claim is admitted afterwards.
//! - `INCOMPLETE`: set by a writer that lost a frame it may still act on
//!   (capacity exhaustion, or abandoning a claimed frame while alive). The
//!   receiver treats the trace as unusable for caching when this is set.
//!   A frame lost to process death deliberately does *not* set this flag:
//!   frames are published before the traced operation is performed, so a
//!   process that died mid-frame never performed the operation.
//! - slot count / reserved payload bytes: the two region frontiers. Claims
//!   move both in one compare-and-swap, so the regions can never overlap and
//!   the receiver's close snapshot counts every admitted slot.
//!
//! This module is pure bit manipulation; the atomic operations applying these
//! values live in [`super::state`].

use super::layout;

pub(super) const CLOSED: u64 = 1 << 63;
pub(super) const INCOMPLETE: u64 = 1 << 62;
const SLOT_COUNT_SHIFT: u32 = 32;
const SLOT_COUNT_MAX: u64 = (1 << 30) - 1;
const PAYLOAD_BYTES_MAX: u64 = u32::MAX as u64;

/// A decoded snapshot of the allocator word.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct AllocWord(u64);

/// Why a claim was not admitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ReserveError {
    /// The receiver has closed the channel.
    Closed,
    /// The frame does not fit: it is larger than [`layout::MAX_PAYLOAD_LEN`],
    /// or the descriptor-table and payload frontiers would meet.
    Capacity,
}

/// A successful reservation of one descriptor slot and one payload span.
#[derive(Debug)]
pub(super) struct Reservation {
    /// The allocator word to install with compare-and-swap.
    pub(super) new_word: AllocWord,
    /// Index of the reserved descriptor slot.
    pub(super) slot_index: usize,
    /// Byte offset of the reserved payload span. Word-aligned.
    pub(super) payload_offset: usize,
}

impl AllocWord {
    pub(super) const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub(super) const fn bits(self) -> u64 {
        self.0
    }

    pub(super) const fn is_closed(self) -> bool {
        self.0 & CLOSED != 0
    }

    pub(super) const fn is_incomplete(self) -> bool {
        self.0 & INCOMPLETE != 0
    }

    pub(super) const fn slot_count(self) -> usize {
        ((self.0 >> SLOT_COUNT_SHIFT) & SLOT_COUNT_MAX) as usize
    }

    pub(super) const fn reserved_payload_bytes(self) -> usize {
        (self.0 & PAYLOAD_BYTES_MAX) as usize
    }

    /// Whether this word describes in-bounds regions of a `mapping_len`-byte
    /// mapping. False only for words a correct writer never produces.
    pub(super) const fn is_valid_for(self, mapping_len: usize) -> bool {
        layout::fits(mapping_len, self.slot_count(), self.reserved_payload_bytes())
    }

    /// Computes the reservation of one slot and a word-aligned span for a
    /// `payload_len`-byte payload, or reports why the claim is not admitted.
    ///
    /// Pure: the caller must install `new_word` with a compare-and-swap
    /// against the word this was computed from.
    pub(super) const fn reserve(
        self,
        payload_len: usize,
        mapping_len: usize,
    ) -> Result<Reservation, ReserveError> {
        if self.is_closed() {
            return Err(ReserveError::Closed);
        }
        if payload_len > layout::MAX_PAYLOAD_LEN {
            return Err(ReserveError::Capacity);
        }
        let slot_index = self.slot_count();
        let new_slot_count = slot_index + 1;
        let new_payload_bytes =
            self.reserved_payload_bytes() + layout::reserved_payload_len(payload_len);
        if new_slot_count as u64 > SLOT_COUNT_MAX
            || new_payload_bytes as u64 > PAYLOAD_BYTES_MAX
            || !layout::fits(mapping_len, new_slot_count, new_payload_bytes)
        {
            return Err(ReserveError::Capacity);
        }
        Ok(Reservation {
            new_word: Self(
                (self.0 & INCOMPLETE)
                    | ((new_slot_count as u64) << SLOT_COUNT_SHIFT)
                    | new_payload_bytes as u64,
            ),
            slot_index,
            // `fits` guarantees `new_payload_bytes <= mapping_len`.
            payload_offset: mapping_len - new_payload_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::*;

    const MAPPING_LEN: usize = 1024;

    #[test]
    fn zero_word_is_open_and_empty() {
        let word = AllocWord::from_bits(0);
        assert!(!word.is_closed());
        assert!(!word.is_incomplete());
        assert!(word.slot_count() == 0);
        assert!(word.reserved_payload_bytes() == 0);
        assert!(word.is_valid_for(layout::HEADER_LEN));
    }

    #[test]
    fn reserve_advances_both_frontiers() {
        let word = AllocWord::from_bits(0);
        let reservation = word.reserve(5, MAPPING_LEN).unwrap();
        assert!(reservation.slot_index == 0);
        assert!(reservation.payload_offset == MAPPING_LEN - 8);
        assert!(reservation.new_word.slot_count() == 1);
        assert!(reservation.new_word.reserved_payload_bytes() == 8);

        let second = reservation.new_word.reserve(9, MAPPING_LEN).unwrap();
        assert!(second.slot_index == 1);
        assert!(second.payload_offset == MAPPING_LEN - 8 - 16);
        assert!(second.new_word.slot_count() == 2);
        assert!(second.new_word.reserved_payload_bytes() == 24);
    }

    #[test]
    fn reserve_rejects_closed() {
        let word = AllocWord::from_bits(CLOSED);
        assert!(word.reserve(1, MAPPING_LEN).unwrap_err() == ReserveError::Closed);
    }

    #[test]
    fn reserve_preserves_incomplete() {
        let word = AllocWord::from_bits(INCOMPLETE);
        let reservation = word.reserve(1, MAPPING_LEN).unwrap();
        assert!(reservation.new_word.is_incomplete());
        assert!(!reservation.new_word.is_closed());
    }

    #[test]
    fn reserve_rejects_oversized_payloads() {
        let word = AllocWord::from_bits(0);
        assert!(
            word.reserve(layout::MAX_PAYLOAD_LEN + 1, usize::MAX).unwrap_err()
                == ReserveError::Capacity
        );
    }

    #[test]
    fn reserve_stops_at_the_frontier_collision() {
        // 80 bytes fit the header, one slot, and one 8-byte payload span.
        let word = AllocWord::from_bits(0);
        let reservation = word.reserve(8, 80).unwrap();
        assert!(reservation.payload_offset == 72);
        assert!(reservation.new_word.reserve(1, 80).unwrap_err() == ReserveError::Capacity);
        // A failed reservation leaves the word untouched by construction:
        // `reserve` is pure and the caller never installs a failed result.
    }

    #[test]
    fn reserve_handles_the_4_gib_mapping_edge() {
        let mapping_len = layout::MAX_MAPPING_LEN;
        let max_payload = mapping_len - layout::HEADER_LEN - layout::SLOT_LEN;
        // One frame cannot exceed MAX_PAYLOAD_LEN even if the mapping has room.
        assert!(
            AllocWord::from_bits(0).reserve(max_payload, mapping_len).unwrap_err()
                == ReserveError::Capacity
        );

        // Fill the payload region with maximal frames until the 32-bit
        // reserved-bytes field would have to exceed its width; the capacity
        // check must fail first.
        let mut word = AllocWord::from_bits(0);
        loop {
            match word.reserve(layout::MAX_PAYLOAD_LEN, mapping_len) {
                Ok(reservation) => {
                    assert!(reservation.new_word.is_valid_for(mapping_len));
                    word = reservation.new_word;
                }
                Err(err) => {
                    assert!(err == ReserveError::Capacity);
                    break;
                }
            }
        }
        assert!(word.slot_count() == 1);
        // The remaining space still admits smaller frames.
        let reservation = word.reserve(1024, mapping_len).unwrap();
        assert!(reservation.new_word.is_valid_for(mapping_len));
    }
}

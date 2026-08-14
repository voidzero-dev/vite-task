//! Encoding of one descriptor slot — the 64-bit value that publishes a frame.
//!
//! ```text
//! bit 63     bits 32..=62               bits 0..=31
//! ABORTED    payload length (31)        payload offset (32)
//! ```
//!
//! | Value                     | State                                       |
//! | ------------------------- | ------------------------------------------- |
//! | `0`                       | Unfinished: slot reserved, nothing published |
//! | `1 << 63`                 | Aborted: the receiver froze the unfinished slot |
//! | nonzero, bit 63 clear     | Committed: offset and length of the payload |
//!
//! Committed lengths are nonzero (a zero-length frame is never claimed), so a
//! committed value is always nonzero and the three states are disjoint.
//! Committed and aborted are terminal: no protocol operation overwrites them.
//!
//! Like [`super::layout`], this module is pure; the compare-and-swap
//! transitions live in [`super::state`].

pub(super) const UNFINISHED: u64 = 0;
pub(super) const ABORTED: u64 = 1 << 63;
const LEN_SHIFT: u32 = 32;
const OFFSET_MAX: u64 = u32::MAX as u64;

/// A decoded descriptor slot value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SlotState {
    /// The slot was reserved but no payload has been committed.
    Unfinished,
    /// The receiver froze the slot; its payload is permanently unreachable.
    Aborted,
    /// A payload was committed. The range is *unvalidated*: it must pass
    /// [`super::layout::PayloadSpan::validate`] before any access.
    Committed { payload_offset: usize, payload_len: usize },
    /// A value no protocol operation produces. The trace is corrupt.
    Corrupt,
}

/// Encodes a committed descriptor.
///
/// The caller guarantees `payload_len` is `1..=MAX_PAYLOAD_LEN` and
/// `payload_offset` fits 32 bits; both hold for any admitted reservation.
pub(super) fn committed(payload_offset: usize, payload_len: usize) -> u64 {
    debug_assert!(payload_len > 0 && payload_len <= super::layout::MAX_PAYLOAD_LEN);
    debug_assert!(payload_offset as u64 <= OFFSET_MAX);
    ((payload_len as u64) << LEN_SHIFT) | payload_offset as u64
}

/// Decodes a slot value read back from shared memory.
pub(super) const fn decode(bits: u64) -> SlotState {
    match bits {
        UNFINISHED => SlotState::Unfinished,
        ABORTED => SlotState::Aborted,
        _ if bits & ABORTED != 0 => SlotState::Corrupt,
        _ => {
            // Bit 63 is clear, so the length field is at most 31 bits and
            // cannot exceed `MAX_PAYLOAD_LEN`.
            let payload_len = (bits >> LEN_SHIFT) as usize;
            let payload_offset = (bits & OFFSET_MAX) as usize;
            if payload_len == 0 {
                // A nonzero offset with a zero length: not a committed value,
                // because committed lengths are nonzero.
                SlotState::Corrupt
            } else {
                SlotState::Committed { payload_offset, payload_len }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::*;

    #[test]
    fn decode_recognizes_the_three_states() {
        assert!(decode(UNFINISHED) == SlotState::Unfinished);
        assert!(decode(ABORTED) == SlotState::Aborted);
        assert!(
            decode(committed(1016, 5))
                == SlotState::Committed { payload_offset: 1016, payload_len: 5 }
        );
    }

    #[test]
    fn committed_roundtrips_the_extremes() {
        let max_offset = u32::MAX as usize;
        let max_len = super::super::layout::MAX_PAYLOAD_LEN;
        assert!(
            decode(committed(max_offset, max_len))
                == SlotState::Committed { payload_offset: max_offset, payload_len: max_len }
        );
        assert!(
            decode(committed(0, 1)) == SlotState::Committed { payload_offset: 0, payload_len: 1 }
        );
    }

    #[test]
    fn decode_rejects_values_no_writer_produces() {
        // Aborted bit combined with other bits.
        assert!(decode(ABORTED | 1) == SlotState::Corrupt);
        assert!(decode(ABORTED | (1 << 62)) == SlotState::Corrupt);
        // Zero length with a nonzero offset.
        assert!(decode(42) == SlotState::Corrupt);
    }
}

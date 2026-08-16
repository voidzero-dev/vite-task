//! Everything the writer and the reader sides share.
//!
//! The region is divided into three fixed areas:
//!
//! ```text
//! | counters | descriptor table | payloads (grow up) |
//! ```
//!
//! Counters and table are one `repr(C)` [`Meta`] struct whose table
//! length is a const parameter the channel specifies; the payload area is
//! the rest of the mapping, so the whole geometry is that struct's size.
//! This module holds the struct, the slot wire format, and
//! [`MappedLayout`]: where the parts of one mapping are, worked out once
//! at attach. The sides live in [`super::writer`] and [`super::reader`].
//!
//! Overflow safety: the payload area is never longer than `u32::MAX`
//! bytes (checked at attach), so an offset and a length always fit a
//! descriptor's 32-bit fields, and bounds checks add them in 32 bits.

use std::{num::NonZeroU32, ptr::NonNull, sync::atomic::AtomicU64};

/// The CLOSED gate bit of the claim counter: set when the receiver seals
/// the channel, and by any failed claim as its loss report (rule 1).
///
/// A bit, not a value to compare against: it survives the `fetch_add` of
/// a writer that arrives late. And a refused claim puts back what it
/// added, so the count stays at or below one per slot plus the claims
/// still running — it can never climb into this bit.
pub(super) const CLOSED: u64 = 1 << 63;

/// The part of the region that is always in the same place — the
/// counters and the descriptor table — as one `repr(C)` struct, which
/// must start zeroed. The payload area is the rest of the mapping.
#[repr(C)]
pub(super) struct Meta<const SLOTS: usize> {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    pub(super) claims: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    pub(super) payload_reserved: AtomicU64,
    /// One descriptor slot per frame.
    pub(super) table: [AtomicU64; SLOTS],
}

// The mapping starts at a `u64`-aligned address and is cast to `&Meta`,
// so no field in `Meta` may need more alignment than that.
const _: () = assert!(align_of::<Meta<0>>() == align_of::<AtomicU64>());

// --- The descriptor slot codec ---------------------------------------------
//
// One slot is a 64-bit value that publishes a frame:
//
// ```text
// bits 32..=63           bits 0..=31
// payload length (32)    payload offset (32)
// ```
//
// | Value                 | State                                           |
// | --------------------- | ----------------------------------------------- |
// | `0`                   | Unfinished: slot reserved, nothing published    |
// | `1`                   | Frozen by the seal: never publishable again     |
// | length field nonzero  | Committed: offset and length of the payload     |
//
// A claimed frame is never zero-length, so a committed value always has a
// nonzero length field and no slot value can mean two of these at once:
// `1` is a zero length with offset `1`, which no writer ever commits. Once
// a slot is committed or frozen, nothing ever changes it again.
//
// Offsets are counted from the start of the payload area, so a descriptor
// can never point into the counters or the table.

pub(super) const UNFINISHED: u64 = 0;

/// The value the seal installs in an unfinished slot: still nothing
/// published (zero length field), but nonzero, so a late commit's
/// compare-and-swap from [`UNFINISHED`] loses.
pub(super) const FROZEN: u64 = 1;

// --- The mapped layout and the ordering contract ---------------------------
// `MappedLayout::new` works out both pointers once, at attach: one to the
// `Meta` struct, one to the payload area. The endpoint keeps them next to
// the mapping they point into. The payload pointer stays raw because
// writers hand out `&mut` slices into it, which must not overlap a shared
// reference.
//
// # Shared atomics
//
// Two independent monotonic `AtomicU64` counters:
//
// - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//   claims. One wait-free `fetch_add` per claim, and the old value it
//   returns says everything the writer needs: which slot it got, whether
//   the channel is closed, and whether the table is full. The seal sets
//   the gate, and so does every failed claim: the one bit both reports
//   the loss and stops later writers doing work for a result the
//   receiver must already reject.
// - the **payload counter**: payload bytes reserved, one `fetch_add`.
//
// A refused claim puts back what it added, always after the gate is set
// — either it saw the gate, or it set the gate itself. Each subtraction
// pairs with that same claim's addition, so the claim counter never
// falls below the number of successful claims, nor below what a seal
// already saw, and never climbs past one per slot plus the claims still
// running: it cannot count up into the gate bit. (A writer that dies
// between adding and putting back leaves one count behind — harmless
// unless it happens 2^63 times.) The payload counter takes back only
// reservations that ran off the end, which no live frame sits above; a
// refused claim that did fit keeps its bytes counted, and no one ever
// writes there. Neither counter says where data is — each descriptor
// carries its own offset and length — and the receiver clamps them
// rather than trusting them.
//
// # Memory-ordering contract
//
// 1. **Claim versus seal** — the seal boundary is a plain snapshot load
//    of the claim counter: claims at or before it in the counter's
//    modification order are in; later ones get slot indices the receiver
//    never visits. Claims publish no payload data, so `Relaxed`
//    suffices. The gate is not the boundary — it only stops late
//    writers; a claim that gets in between the snapshot and the gate
//    lands past the snapshot, where the receiver never looks.
//    Completeness rides the same modification order: a failed claim sets
//    the gate before performing the operation whose record was lost, so
//    either the snapshot sees the bit or the loss happened after the
//    boundary; a writer that skipped on seeing the bit is covered the
//    same way; one that died before setting it never performed its
//    operation, so nothing was lost.
// 2. **Writer commit** — `FrameMut::finish`'s compare-and-swap uses
//    `Release`: every payload write happens-before the descriptor is
//    visible.
// 3. **Receiver observation** — the freeze compare-and-swap in
//    `ShmReader::seal` uses `Acquire` on failure: an observed descriptor
//    implies fully visible payload bytes.

/// Converts an integer into a `usize`.
///
/// Never loses bits: the argument has to fit a `u64`, which is what the
/// bound says, and the assert lets only targets whose `usize` is 64 bits
/// build this module. That assert is the only reason the cast is safe,
/// so it sits here rather than at module scope — and this is the only
/// `as` in the protocol.
#[expect(clippy::cast_possible_truncation, reason = "the assert allows only equal widths")]
pub(super) fn to_usize(value: impl Into<u64>) -> usize {
    const { assert!(size_of::<usize>() == size_of::<u64>(), "requires a 64-bit target") };
    value.into() as usize
}

/// Casts to a pointer of another type, returning `None` when the pointer
/// is not aligned for `U`: a stable stand-in for the still-unstable
/// [`<*mut T>::try_cast_aligned`][std].
///
/// [std]: https://doc.rust-lang.org/std/primitive.pointer.html#method.try_cast_aligned
fn try_cast_aligned<T, U>(ptr: *mut T) -> Option<*mut U> {
    if ptr.addr().is_multiple_of(align_of::<U>()) { Some(ptr.cast()) } else { None }
}

/// Where the two parts of the region are: the fixed [`Meta`] struct and
/// the payload area. Worked out once at attach and kept in the endpoint.
/// The pointers stay valid as long as the mapping does, because they
/// point into the mapped memory, not into the endpoint holding them.
#[derive(Clone, Copy)]
pub(super) struct MappedLayout<const SLOTS: usize> {
    meta: NonNull<Meta<SLOTS>>,
    /// Start of the payload area. A raw pointer, not a reference:
    /// writers hand out `&mut` slices into it, which must not overlap a
    /// shared reference.
    pub(super) payloads: NonNull<u8>,
    /// Length of the payload area. A `u32`, the width of a descriptor's
    /// offset and length, so bounds checks need no conversion.
    payload_len: u32,
}

/// A decoded descriptor slot (the codec above).
#[derive(Clone, Copy)]
pub(super) enum SlotState {
    /// Nothing is published in the slot: the writer has not finished it
    /// yet, died or abandoned it before finishing, or the seal froze it
    /// ([`FROZEN`]) so it never can be. The receiver ignores such slots.
    Unfinished,
    /// A payload is committed: the receiver may read its span, once
    /// checked against the payload area's bounds.
    Committed {
        /// Byte offset of the payload from the start of the payload region.
        offset: u32,
        /// Byte length of the payload. Nonzero, so that a committed value
        /// can never collide with [`UNFINISHED`] or [`FROZEN`] — the
        /// offset alone could be zero.
        len: NonZeroU32,
    },
}

impl SlotState {
    /// Decodes a slot value into its state. The two processes sharing a
    /// value run on one machine, so native byte order is fine.
    pub(super) const fn decode(slot_value: u64) -> Self {
        let [offset, len] = bytemuck::must_cast::<u64, [u32; 2]>(slot_value);
        let Some(len) = NonZeroU32::new(len) else {
            return Self::Unfinished;
        };
        Self::Committed { offset, len }
    }

    /// Encodes this state as a slot value; the inverse of [`Self::decode`].
    pub(super) const fn encode(self) -> u64 {
        let [offset, len] = match self {
            Self::Unfinished => [0, 0],
            Self::Committed { offset, len } => [offset, len.get()],
        };
        bytemuck::must_cast([offset, len])
    }
}

impl<const SLOTS: usize> MappedLayout<SLOTS> {
    /// Locates the parts of a shared mapping, or returns `None` when the
    /// mapping cannot hold the protocol: a null or misaligned start, too
    /// little room for [`Meta`], or a payload area too long for a 32-bit
    /// offset to reach.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes, and its address stable,
    ///   for as long as the returned pointers (and any copy of them) are
    ///   used.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    pub(super) unsafe fn new(mem: *mut [u8]) -> Option<Self> {
        let mem_start = mem.cast::<u8>();
        // The mapping must hold the fixed struct.
        let payload_len = mem.len().checked_sub(size_of::<Meta<SLOTS>>())?;
        // A descriptor holds a 32-bit offset, so the payload area can be
        // no longer than a `u32`. Keeping the converted value is what lets
        // later bounds checks stay in 32 bits.
        let payload_len = u32::try_from(payload_len).ok()?;
        // These two conversions are the checks on the start address:
        // aligned for `Meta`, and not null.
        let meta = NonNull::new(try_cast_aligned::<_, Meta<SLOTS>>(mem_start)?)?;

        // The payload area is everything after the fixed struct.
        // SAFETY: the mapping holds the struct (checked above).
        let payloads = NonNull::new(unsafe { mem_start.add(size_of::<Meta<SLOTS>>()) })?;
        Some(Self { meta, payloads, payload_len })
    }

    /// The fixed part of the region.
    const fn meta(&self) -> &Meta<SLOTS> {
        // SAFETY: `new`'s contract keeps the memory valid while any
        // pointer is used, and `Meta` is all atomics, so the shared
        // borrow is valid even while other threads and processes access
        // the same memory through them.
        unsafe { self.meta.as_ref() }
    }

    /// The claim counter.
    pub(super) const fn claims(&self) -> &AtomicU64 {
        &self.meta().claims
    }

    /// The payload counter.
    pub(super) const fn payload_reserved(&self) -> &AtomicU64 {
        &self.meta().payload_reserved
    }

    /// The descriptor table.
    pub(super) const fn table(&self) -> &[AtomicU64] {
        &self.meta().table
    }

    /// Whether `len` bytes at `offset` lie inside the payload area. Both
    /// sides ask this: the writer about the span it just reserved, the
    /// reader about every descriptor it decodes.
    pub(super) const fn holds_span(&self, offset: u32, len: u32) -> bool {
        match offset.checked_add(len) {
            Some(end) => end <= self.payload_len,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use assert2::assert;

    use super::*;

    #[test]
    fn slot_codec_roundtrips() {
        for (offset, len) in [(0, 5), (3, 5), (u32::MAX, 1), (0, u32::MAX)] {
            let len = NonZeroU32::new(len).unwrap();
            let encoded = SlotState::Committed { offset, len }.encode();
            let SlotState::Committed { offset: o, len: l } = SlotState::decode(encoded) else {
                panic!("committed value decoded as unfinished");
            };
            assert!(o == offset && l == len);
        }
        // Zero length fields publish nothing, whatever the offset half says.
        assert!(matches!(SlotState::decode(UNFINISHED), SlotState::Unfinished));
        assert!(matches!(SlotState::decode(FROZEN), SlotState::Unfinished));
        assert!(matches!(SlotState::decode(42), SlotState::Unfinished));
    }

    #[test]
    fn meta_is_the_counters_then_the_table() {
        assert!(size_of::<Meta<15>>() == (2 + 15) * size_of::<AtomicU64>());
        assert!(align_of::<Meta<15>>() == align_of::<AtomicU64>());
    }
}

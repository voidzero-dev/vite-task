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
/// A bit, not a value to compare against, so it survives the increment of
/// a writer that arrives late. Counting cannot realistically reach it: at
/// the rate claims are measured to run, 2^63 of them take thousands of
/// years, and a channel lives for one command. If it ever did happen the
/// gate would read as set — no more records, and the seal fails, which is
/// the cautious answer rather than a wrong one.
pub const CLOSED: u64 = 1 << 63;

/// The part of the region that is always in the same place — the
/// counters and the descriptor table — as one `repr(C)` struct, which
/// must start zeroed. The payload area is the rest of the mapping.
#[repr(C)]
pub struct Meta<const SLOTS: usize> {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    pub claims: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    pub payload_reserved: AtomicU64,
    /// One descriptor slot per frame.
    pub table: [AtomicU64; SLOTS],
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
// | `0`                   | Unfinished: slot claimed, nothing published yet |
// | length field nonzero  | Committed: offset and length of the payload     |
//
// A claimed frame is never zero-length, so a committed value always has a
// nonzero length field and can never be read as the zero a fresh slot
// starts at. One writer owns each slot and writes it once, so a slot goes
// from zero to committed and never changes again.
//
// Offsets are counted from the start of the payload area, so a descriptor
// can never point into the counters or the table.

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
//   claims. One increment per claim, and the old value it returns says
//   everything the writer needs: which slot it got, whether the channel
//   is closed, and whether the table is full. The seal sets the gate, and
//   so does every failed claim: the one bit both reports the loss and
//   stops later writers doing work for a result the receiver must
//   already reject.
// - the **payload counter**: payload bytes reserved, one increment.
//
// Both counters only ever climb, and a refused claim leaves its increment
// behind. That costs nothing: neither counter says where data is — each
// descriptor carries its own offset and length — so an inflated one
// points nothing at the wrong bytes. The receiver clamps its snapshot to
// the table length rather than trusting it.
//
// Neither add is guarded against wrapping, because neither wrap can be
// reached. The payload counter can only pass the region once a claim has
// failed, which sets the gate — and from then on every claim is refused
// at the claim counter, which is read after the reservation and before
// any span is built, so a wrapped payload counter never gets to name an
// offset. The claim count reaching the gate bit needs 2^63 claims, which
// is thousands of years at the rate they are measured to run; a channel
// lives for one command, and a gate that read as set by accident would
// only make the seal fail.
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
// 2. **Writer commit** — `FrameMut::finish` stores the descriptor with
//    `Release`: every payload write happens-before it can be seen. The
//    writer that claimed the slot is the only one that ever writes it, so
//    a plain store is enough.
// 3. **Receiver read** — `Iter` loads each descriptor with `Acquire`, so
//    a descriptor it sees brings the payload bytes with it.

/// Converts an integer into a `usize`.
///
/// Never loses bits: the argument has to fit a `u64`, which is what the
/// bound says, and the assert lets only targets whose `usize` is 64 bits
/// build this module. That assert is the only reason the cast is safe,
/// so it sits here rather than at module scope — and this is the only
/// `as` in the protocol.
#[expect(clippy::cast_possible_truncation, reason = "the assert allows only equal widths")]
pub fn to_usize(value: impl Into<u64>) -> usize {
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
pub struct MappedLayout<const SLOTS: usize> {
    meta: NonNull<Meta<SLOTS>>,
    /// Start of the payload area. A raw pointer, not a reference:
    /// writers hand out `&mut` slices into it, which must not overlap a
    /// shared reference.
    pub payload_start: NonNull<u8>,
    /// Length of the payload area. A `u32`, the width of a descriptor's
    /// offset and length, so the writer's bounds check needs no
    /// conversion.
    pub payload_len: u32,
}

/// A decoded descriptor slot (the codec above).
#[derive(Clone, Copy)]
pub enum SlotState {
    /// Nothing is published in the slot: the writer has not finished it
    /// yet, or died or gave it up before finishing. The receiver ignores
    /// such slots.
    Unfinished,
    /// A payload is committed: the receiver may read its span, once
    /// checked against the payload area's bounds.
    Committed {
        /// Byte offset of the payload from the start of the payload region.
        offset: u32,
        /// Byte length of the payload. Nonzero, so a committed value is
        /// never the zero a fresh slot starts at — the offset alone could
        /// be zero.
        len: NonZeroU32,
    },
}

impl SlotState {
    /// Decodes a slot value into its state. The two processes sharing a
    /// value run on one machine, so native byte order is fine.
    pub const fn decode(slot_value: u64) -> Self {
        let [offset, len] = bytemuck::must_cast::<u64, [u32; 2]>(slot_value);
        let Some(len) = NonZeroU32::new(len) else {
            return Self::Unfinished;
        };
        Self::Committed { offset, len }
    }

    /// Encodes this state as a slot value; the inverse of [`Self::decode`].
    pub const fn encode(self) -> u64 {
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
    pub unsafe fn new(mem: *mut [u8]) -> Option<Self> {
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
        let payload_start = NonNull::new(unsafe { mem_start.add(size_of::<Meta<SLOTS>>()) })?;
        Some(Self { meta, payload_start, payload_len })
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
    pub const fn claims(&self) -> &AtomicU64 {
        &self.meta().claims
    }

    /// The payload counter.
    pub const fn payload_reserved(&self) -> &AtomicU64 {
        &self.meta().payload_reserved
    }

    /// The descriptor table.
    pub const fn table(&self) -> &[AtomicU64] {
        &self.meta().table
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
        assert!(matches!(SlotState::decode(0), SlotState::Unfinished));
        assert!(matches!(SlotState::decode(42), SlotState::Unfinished));
    }

    #[test]
    fn meta_is_the_counters_then_the_table() {
        assert!(size_of::<Meta<15>>() == (2 + 15) * size_of::<AtomicU64>());
        assert!(align_of::<Meta<15>>() == align_of::<AtomicU64>());
    }
}

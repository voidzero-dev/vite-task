//! What the writer and reader sides share: the region's `repr(C)` shape,
//! the descriptor format, and [`MappedLayout`], which locates the parts of
//! one mapping. The sides live in [`super::writer`] and [`super::reader`];
//! `README.md` describes the region itself.
//!
//! The payload area never exceeds `u32::MAX` bytes, checked at attach, so
//! an offset and a length always fit a descriptor's 32-bit fields and
//! bounds checks add them in 32 bits.

use std::{num::NonZeroU32, ptr::NonNull, sync::atomic::AtomicU64};

/// The CLOSED gate bit of the claim counter. The receiver sets it when it
/// seals, and so does a claim the region had no room for, which is how it
/// reports the loss (rule 1). Nothing else sets it: what a writer cannot
/// record for its own reasons is not this protocol's to report.
///
/// A bit rather than a value to compare against, so it survives the
/// increment of a writer that arrives late. Counting cannot reach it: that
/// takes 2^63 claims, and a channel lives for one command. A gate that
/// somehow read as set would fail the seal, which is the cautious
/// answer.
pub const CLOSED: u64 = 1 << 63;

/// The two counters at the start of the region, as one `repr(C)` struct
/// that starts zeroed. The descriptor table follows them, and payloads
/// take the rest of the mapping.
#[repr(C)]
pub struct Counters {
    /// Bit 63 is the CLOSED gate. The low bits count claims that got as
    /// far as reserving payload space, which is where a slot is taken.
    pub claims: AtomicU64,
    /// Payload bytes ever reserved, failed claims included.
    pub payload_reserved: AtomicU64,
}

// The mapping starts at a `u64`-aligned address and is cast to
// `&Counters`, so no field in it may need more alignment than that. The
// descriptor table then starts at a multiple of that alignment too.
const _: () = assert!(align_of::<Counters>() == align_of::<AtomicU64>());

// Both endpoints keep raw pointers into the region and read through them
// long after the references they came from are gone, which is sound only
// because every byte they point at sits inside an atomic. This catches a
// field being added or padding appearing; it cannot catch a field changing
// type, so keep that in mind when editing the struct.
const _: () = assert!(size_of::<Counters>() == 2 * size_of::<AtomicU64>());

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

// --- MappedLayout and the ordering contract --------------------------------
// `MappedLayout::new` works out where each part sits once, at attach, and
// the endpoint keeps the result beside the mapping. The payload pointer
// stays raw because writers hand out `&mut` slices into it, which must not
// overlap a shared reference.
//
// Neither counter guards its add against wrapping, because neither wrap is
// reachable. The payload counter passes the region only after a claim has
// failed, and every claim after that is refused before it builds a span.
// The claim count needs 2^63 increments to reach the gate bit.
//
// # Memory-ordering contract
//
// 1. **Claim versus seal.** The seal swaps the gate into the claim counter
//    and reads the old value in one step, so the boundary and the gate are
//    one point in that counter's modification order: claims at or before
//    it are in, every later one fails on the gate. Claims publish no
//    payload data, so `Relaxed` suffices. Completeness rides the same
//    order: a failed claim sets the gate before performing the operation
//    whose record it lost, so either the seal sees the bit or the loss
//    happened past the boundary. A writer that died before setting it
//    never performed its operation.
// 2. **Writer commit.** `FrameMut::finish` stores the descriptor with
//    `Release`, so every payload write lands first. The writer that
//    claimed the slot is the only one that writes it, so a store is
//    enough.
// 3. **Receiver read.** `Iter` loads each descriptor with `Acquire`, so a
//    descriptor it sees brings the payload bytes along.

/// Requires the target's `usize` to be as wide as a `u64`, which is what
/// makes [`to_usize`] and [`from_usize`] lossless. Stated once, and cited
/// by both.
const EQUAL_WIDTHS: () =
    assert!(size_of::<usize>() == size_of::<u64>(), "requires a 64-bit target");

/// Converts an integer into a `usize`.
///
/// Never loses bits: the bound takes only what fits a `u64`, and
/// [`EQUAL_WIDTHS`] lets only targets whose `usize` is that wide build
/// this module. Those asserts are why the casts are safe, so the pair sits
/// here rather than at module scope. They are the only `as` in the
/// protocol.
#[expect(clippy::cast_possible_truncation, reason = "the assert allows only equal widths")]
pub fn to_usize(value: impl Into<u64>) -> usize {
    const { EQUAL_WIDTHS };
    value.into() as usize
}

/// Converts a `usize` into a `u64`; the inverse of [`to_usize`].
#[expect(clippy::as_conversions, reason = "the assert allows only equal widths")]
pub const fn from_usize(value: usize) -> u64 {
    const { EQUAL_WIDTHS };
    value as u64
}

/// Casts to a pointer of another type, returning `None` when the pointer
/// is not aligned for `U`: a stable stand-in for the still-unstable
/// [`<*mut T>::try_cast_aligned`][std].
///
/// [std]: https://doc.rust-lang.org/std/primitive.pointer.html#method.try_cast_aligned
fn try_cast_aligned<T, U>(ptr: *mut T) -> Option<*mut U> {
    if ptr.addr().is_multiple_of(align_of::<U>()) { Some(ptr.cast()) } else { None }
}

/// Where the counters, the descriptor table and the payload area sit in
/// one mapping, worked out once at attach. The pointers outlive this
/// value, since they point into the mapping rather than into the endpoint
/// holding them.
#[derive(Clone, Copy)]
pub struct MappedLayout {
    counters: NonNull<Counters>,
    /// Every descriptor slot the region was created with.
    table: NonNull<[AtomicU64]>,
    /// Start of the payload area, raw rather than a reference: writers
    /// hand out `&mut` slices into it, which must not overlap a shared
    /// reference.
    pub payload_start: NonNull<u8>,
    /// Length of the payload area, in the width of a descriptor's offset,
    /// so the writer's bounds check needs no conversion.
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
        /// never the zero a fresh slot starts at, which the offset alone
        /// could be.
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

impl MappedLayout {
    /// Locates the parts of a shared mapping whose table holds `slots`
    /// descriptors, or returns `None` when the mapping cannot hold the
    /// protocol: a null or misaligned start, too little room for the
    /// counters and that many slots, or a payload area too long for a
    /// 32-bit offset to reach.
    ///
    /// Both endpoints must pass the `slots` the region was created with.
    /// A smaller one reads part of the table as payload; a larger one
    /// reads payload bytes as descriptors.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes, and its address stable,
    ///   for as long as the returned pointers (and any copy of them) are
    ///   used.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    pub unsafe fn new(mem: *mut [u8], slots: usize) -> Option<Self> {
        let mem_start = mem.cast::<u8>();
        // The mapping must hold the counters and the whole table.
        let table_len = slots.checked_mul(size_of::<AtomicU64>())?;
        let meta_len = size_of::<Counters>().checked_add(table_len)?;
        let payload_len = mem.len().checked_sub(meta_len)?;
        // A descriptor holds a 32-bit offset, so the payload area can be
        // no longer than a `u32`. Keeping the converted value is what lets
        // later bounds checks stay in 32 bits.
        let payload_len = u32::try_from(payload_len).ok()?;
        // These two conversions are the checks on the start address:
        // aligned for `Counters`, and not null.
        let counters = NonNull::new(try_cast_aligned::<_, Counters>(mem_start)?)?;

        // The table sits right after the counters, and the payload area
        // after the table.
        // SAFETY: the mapping holds both (checked above), and the table's
        // alignment follows from the start address being aligned for
        // `Counters`, whose size is a multiple of that alignment.
        let table_start = NonNull::new(unsafe { mem_start.add(size_of::<Counters>()) })?;
        let table = NonNull::slice_from_raw_parts(table_start.cast::<AtomicU64>(), slots);
        // SAFETY: as above.
        let payload_start = NonNull::new(unsafe { mem_start.add(meta_len) })?;
        Some(Self { counters, table, payload_start, payload_len })
    }

    /// The counters at the start of the region.
    const fn counters(&self) -> &Counters {
        // SAFETY: `new`'s contract keeps the memory valid while any
        // pointer is used, and both counters are atomics, so the shared
        // borrow is valid even while other threads and processes access
        // the same memory through them.
        unsafe { self.counters.as_ref() }
    }

    /// The claim counter.
    pub const fn claims(&self) -> &AtomicU64 {
        &self.counters().claims
    }

    /// The payload counter.
    pub const fn payload_reserved(&self) -> &AtomicU64 {
        &self.counters().payload_reserved
    }

    /// The descriptor table.
    pub const fn table(&self) -> &[AtomicU64] {
        // SAFETY: see `counters`; the table is a run of atomics inside the
        // same mapping.
        unsafe { self.table.as_ref() }
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
    fn a_region_too_small_for_the_table_is_rejected() {
        let mut mem = [0u64; 4];
        let raw = std::ptr::slice_from_raw_parts_mut(mem.as_mut_ptr().cast::<u8>(), 32);
        // Two counters and two slots exactly fill it; a third slot does not.
        // SAFETY: the array is live, aligned and zeroed, and nothing else
        // touches it.
        assert!(unsafe { MappedLayout::new(raw, 2) }.is_some());
        // SAFETY: as above.
        assert!(unsafe { MappedLayout::new(raw, 3) }.is_none());
    }
}

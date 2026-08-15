//! Everything the writer and the reader sides share.
//!
//! The region is divided into three fixed areas:
//!
//! ```text
//! | counters | descriptor table | payloads (grow up) |
//! ```
//!
//! The counters and the table have compile-time shape: one `repr(C)`
//! [`Meta`] struct whose table length is a const parameter the channel
//! specifies. The payload area is simply the rest of the mapping, so the
//! whole geometry reduces to that struct's size. This module holds the
//! struct, the descriptor-slot wire format, and [`MappedLayout`]: the
//! views bound to one concrete mapping, built once
//! when an endpoint attaches. The sides themselves live in
//! [`super::writer`] and [`super::reader`].
//!
//! Overflow safety follows from one bound enforced at attach time: the
//! payload region fits `u32`, so all offsets fit the 32-bit descriptor
//! fields and all sums fit `usize` on the 64-bit targets the parent
//! module asserts.

use std::{num::NonZeroU32, ptr::NonNull, sync::atomic::AtomicU64};

/// The CLOSED gate bit of the claim counter, set when the receiver seals
/// the channel and by any writer whose claim failed — the loss report
/// that also condemns the channel (rule 1 below).
///
/// The gate must be a bit, not a sentinel value: stragglers keep
/// `fetch_add`ing the counter after it is set, and an OR-ed bit survives
/// 2^63 increments, while any exact value would be destroyed by the
/// first. No realistic claim volume carries into bit 63.
pub(super) const CLOSED: u64 = 1 << 63;

/// The region's fixed-location part — the protocol counters and the
/// descriptor table — as one `repr(C)` struct, which must start zeroed.
/// The payload area is simply the rest of the mapping.
#[repr(C)]
pub(super) struct Meta<const SLOTS: usize> {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    pub(super) claims: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    pub(super) payload_reserved: AtomicU64,
    /// One descriptor slot per frame.
    pub(super) table: [AtomicU64; SLOTS],
}

// The `u64`-aligned mapping base is cast to `&Meta`; nothing in it may
// raise the alignment.
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
// Committed lengths are nonzero (a zero-length frame is never claimed), so
// a committed value's length field is nonzero and the three states are
// disjoint: `1` is a zero length with offset `1`, which no writer commits.
// Once a slot is committed or frozen, nothing ever changes it again.
//
// Offsets are measured from the start of the payload area, so no
// descriptor can even name the header or the table.
//
// This defines the format both sides must agree on; encoding lives with
// the writer ([`super::writer`]) and decoding with the reader
// ([`super::reader`]).

pub(super) const UNFINISHED: u64 = 0;

/// The terminal value the seal installs in an unfinished slot: still
/// nothing published (a zero length field), but no longer zero, so a late
/// commit's compare-and-swap from [`UNFINISHED`] loses.
pub(super) const FROZEN: u64 = 1;

// --- The mapped layout and the ordering contract ---------------------------
// `MappedLayout::new` builds two typed views of the region — the
// `repr(C)` `Meta` struct (counters and descriptor table) and the untyped
// payload area as a raw slice — once, when an endpoint attaches; the
// endpoint stores them beside the mapping they point into. Every access
// after that is a plain field access or a bounds-checked index. The
// payload area stays raw because writers hold exclusive `&mut` borrows
// into it, which must not alias any shared reference.
//
// # Shared atomics
//
// The region starts with two independent monotonic `AtomicU64` counters:
//
// - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//   claims ever attempted. Claiming is one wait-free `fetch_add`; the
//   returned old value carries the claim's slot index, the gate, and — by
//   comparison against the fixed table capacity — the capacity verdict.
//   The gate is set by the receiver's seal and by every failed claim:
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
// 1. **Claim versus seal** — the receiver's seal boundary is a plain
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
//    `ShmReader::seal` uses `Acquire` on failure: observing a committed
//    descriptor also makes the payload writes it published visible, so the
//    borrows `ShmReader` later hands out read settled bytes.

/// Casts to a pointer of another type, returning `None` when the pointer
/// is not aligned for `U`: a stable stand-in for the still-unstable
/// [`<*mut T>::try_cast_aligned`][std].
///
/// [std]: https://doc.rust-lang.org/std/primitive.pointer.html#method.try_cast_aligned
fn try_cast_aligned<T, U>(ptr: *mut T) -> Option<*mut U> {
    if ptr.addr().is_multiple_of(align_of::<U>()) { Some(ptr.cast()) } else { None }
}

/// The typed views of the region: the fixed-location [`Meta`] struct
/// and the raw payload area. Built once when an endpoint attaches and
/// stored in it; the views stay valid because they point into the
/// mapping's stable target, not into the endpoint value.
#[derive(Clone, Copy)]
pub(super) struct MappedLayout<const SLOTS: usize> {
    meta: NonNull<Meta<SLOTS>>,
    pub(super) payloads: *mut [u8],
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
    /// Builds the typed views of a shared mapping, or `None` when the
    /// mapping cannot host the protocol at all: base null or not
    /// `u64`-aligned, [`Meta`] not fitting inside the mapping, or the
    /// payload area beyond the descriptors' 32-bit offsets.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes, and its address stable,
    ///   for as long as the returned views (and any copy of them) are
    ///   used.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    pub(super) unsafe fn new(mem: *mut [u8]) -> Option<Self> {
        let mem_start = mem.cast::<u8>();
        let mem_len = mem.len();
        // The mapping must be large enough to hold the fixed-location struct.
        let payload_len = mem_len.checked_sub(size_of::<Meta<SLOTS>>())?;
        // Descriptors store payload offsets in 32 bits: the conversion is
        // the bound on the payload area.
        u32::try_from(payload_len).ok()?;
        // The pointer conversions are the base checks: aligned, non-null.
        let meta = NonNull::new(try_cast_aligned::<_, Meta<SLOTS>>(mem_start)?)?;

        // The payload area is everything after the fixed-location struct.
        // SAFETY: the mapping is large enough to hold it (checked above).
        let payload_start = unsafe { mem_start.add(size_of::<Meta<SLOTS>>()) };

        let payloads = std::ptr::slice_from_raw_parts_mut(payload_start, payload_len);
        Some(Self { meta, payloads })
    }

    /// The fixed-location part of the region.
    const fn meta(&self) -> &Meta<SLOTS> {
        // SAFETY: `new`'s contract keeps the target valid while any view
        // is used, and `Meta` consists entirely of atomics, so the shared
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

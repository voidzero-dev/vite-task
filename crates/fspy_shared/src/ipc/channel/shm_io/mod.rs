//! A crash-tolerant, nonblocking frame channel in a shared memory region.
//!
//! Multiple writer processes append variable-length frames concurrently; one
//! receiver closes the channel and collects every committed frame without
//! waiting for any writer. A process may die at any instruction — mid-claim,
//! mid-write, pre-commit — and only its own unfinished frame is lost.
//!
//! `README.md` in this directory tells the whole story in plain words and
//! indexes the modules.
//!
//! # Region layout
//!
//! ```text
//! low addresses                                            high addresses
//! +--------+--------+--------+---------+-----------+-----------+------+
//! | header | slot 0 | slot 1 | ...     | payload 0 | payload 1 | ...  |
//! +--------+--------+--------+---------+-----------+-----------+------+
//!            fixed descriptor table      payloads grow up ->
//! ```
//!
//! The layout is derived from the mapping length alone ([`layout`]), so
//! the region is self-describing: every process computes the same table
//! and payload bounds from the mapped size. One borrow constructs typed
//! views of the header (a `repr(C)` struct of two monotonic `AtomicU64`
//! counters — claims, carrying the CLOSED gate bit, and payload bytes
//! reserved — plus a loss flag) and of the descriptor table (a slice of
//! atomics); the payload area stays untyped bytes.
//! A claim is two wait-free `fetch_add`s — one reserves payload bytes, one
//! reserves a descriptor slot — validated against the fixed region bounds
//! from the returned old values. A failed claim sets the loss flag and
//! leaves the counters bumped, harmlessly: readers clamp to the region
//! capacities, and committed descriptors are self-describing ([`layout`]),
//! so the counters never locate data. Every slot has a fixed location, so
//! an unfinished frame can never hide a later one.
//!
//! # Frame lifecycle
//!
//! ```text
//!                          writer commit CAS wins
//!                     +-----------------------------> COMMITTED (readable)
//! CLAIMED (slot 0) ---+
//!                     +-----------------------------> ABORTED (ignored)
//!                          receiver freeze CAS wins
//! ```
//!
//! A payload becomes reachable only through its committed descriptor, and a
//! descriptor is committed only after the payload is fully written
//! (the ordering contract below). The receiver never derives frame
//! locations from payload bytes, and the borrows [`ShmReader`] hands out cover
//! exactly the validated committed spans — immutable under the protocol,
//! and disjoint from everything a live writer may still touch (see the
//! receiver section's trust argument below).
//!
//! # Close boundary
//!
//! [`ShmReader::close`]'s boundary is a snapshot of the claim counter.
//! A writer admitted before the snapshot races the freeze pass per slot and
//! its frame is either included (commit won) or ignored (abort won) — never
//! torn; a claim after the snapshot lands in a slot the receiver never
//! visits and is dropped, and the CLOSED gate set before close returns
//! stops stragglers from claiming (and materializing pages) forever. Both
//! drops are sound because writers publish a record *before* performing the
//! recorded operation: a process that died mid-frame never performed the
//! operation, and one that claimed or committed after the snapshot performs
//! it outside the channel's boundary. A record refused *before* close — a
//! full region, an oversized frame — sets the loss flag first, and the
//! channel reports itself incomplete ([`ShmReader::is_complete`]).
//!
//! Correctness never depends on writer-side cleanup: no exit hooks, PID
//! checks, heartbeats, or timeouts.

mod layout;

use std::{
    fmt,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    ptr::slice_from_raw_parts_mut,
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use fspy_shm::Mapping;
use layout::{CLOSED, Header, PayloadSpan};

// The region arithmetic in `layout` relies on `usize` accommodating sums of
// 32-bit-bounded quantities, and the descriptor protocol on native 64-bit
// atomics.
const _: () = assert!(
    size_of::<usize>() >= size_of::<u64>(),
    "the shared-memory frame protocol requires a 64-bit target"
);

/// A trait to borrow a raw memory region.
pub trait AsRawSlice {
    fn as_raw_slice(&self) -> *mut [u8];
}

impl AsRawSlice for Mapping {
    fn as_raw_slice(&self) -> *mut [u8] {
        slice_from_raw_parts_mut(self.as_ptr(), self.len())
    }
}

/// Whether a mapping of `len` bytes can host the protocol at all.
///
/// Senders opening a file they do not control should refuse unsupported
/// lengths with an error; the protocol's own constructors treat them as a
/// broken caller and panic.
#[must_use]
pub fn is_supported_region_len(len: usize) -> bool {
    (layout::HEADER_LEN..=layout::MAX_MAPPING_LEN).contains(&len)
}

// --- The region views and the ordering contract ----------------------------
// One unsafe borrow in `SharedState::borrow` constructs three typed
// views of the region — the `repr(C)` `Header`, the descriptor table as
// a slice of atomics sized by the mapping length, and the untyped payload
// area as a raw slice. Every access after that is a plain field access or
// a bounds-checked index. The payload area stays raw because writers hold
// exclusive `&mut` borrows into it, which must not alias any shared
// reference.
//
// # Shared atomics
//
// The header holds two independent monotonic `AtomicU64` counters and a
// loss flag:
//
// - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//   claims ever attempted. Claiming is one wait-free `fetch_add`; the
//   returned old value carries the claim's slot index, the gate, and — by
//   comparison against the fixed table capacity — the capacity verdict.
// - the **payload counter**: payload bytes ever reserved, bumped by another
//   wait-free `fetch_add`.
// - the **loss flag**: set by every failed claim before the writer moves
//   on. The receiver derives completeness from it alone.
//
// Failed claims leave the counters bumped; that is harmless, because the
// receiver clamps instead of trusting the counts, and committed
// descriptors carry their own offset and length, so the counters never
// locate data.
//
// # Memory-ordering contract
//
// Three synchronization rules cover the whole protocol:
//
// 1. **Claim versus close** — the receiver's close boundary is a plain
//    snapshot load of the claim counter: claims ordered at or before the
//    value it reads (in the counter's modification order) are in the
//    snapshot; later ones receive slot indices the receiver never visits.
//    Claims publish no payload data, so `Relaxed` suffices throughout.
//    The CLOSED gate only stops stragglers from claiming (and allocating
//    pages) forever; any claim admitted between the snapshot and the gate
//    lands beyond the snapshot and is never observed. Completeness rides
//    the same style of argument: a failed claim sets the loss flag before
//    the writer performs the operation whose record was lost — so the
//    receiver's one read of the flag either sees the loss, or the loss
//    belongs to an operation performed after the boundary. A writer that
//    dies before setting the flag never performed its operation, so
//    nothing was actually lost.
// 2. **Writer commit** — the slot compare-and-swap in `FrameMut::finish`
//    uses `Release`: every payload write happens-before the committed
//    descriptor becomes visible.
// 3. **Receiver observation** — the freeze compare-and-swap in
//    `ShmReader::close` uses `Acquire` on failure: observing a committed
//    descriptor also makes the payload writes it published visible, so the
//    borrows `ShmReader` later hands out read settled bytes.

/// A borrowed view of the shared mapping: the typed header, the
/// descriptor table sized from the mapping length, and the raw payload
/// area.
#[derive(Clone, Copy)]
struct SharedState<'m> {
    header: &'m Header,
    table: &'m [AtomicU64],
    payloads: *mut [u8],
    /// The real mapping length. Not derivable from the parts above: the
    /// payload region rounds down to whole `u64`s, and re-deriving the
    /// layout from a shortened length could shift the table boundary.
    len: usize,
}

impl SharedState<'_> {
    /// Borrows a shared mapping.
    ///
    /// # Safety
    ///
    /// - `mem` must be valid for reads and writes for the lifetime `'m` and
    ///   its address must be stable.
    /// - The memory must have been zero-initialized when the region was
    ///   created, and accessed only through this protocol since.
    ///
    /// # Panics
    ///
    /// Panics when the mapping cannot host the protocol at all: base not
    /// `u64`-aligned, smaller than the header, or larger than
    /// [`layout::MAX_MAPPING_LEN`]. These indicate a broken caller, not
    /// runtime data; senders guard untrusted mappings with
    /// [`is_supported_region_len`] first.
    #[expect(clippy::cast_ptr_alignment, reason = "the base is asserted `u64`-aligned below")]
    unsafe fn borrow(mem: *mut [u8]) -> Self {
        let base = mem.cast::<u8>();
        let len = mem.len();
        assert!(base.addr().is_multiple_of(align_of::<Header>()));
        assert!((layout::HEADER_LEN..=layout::MAX_MAPPING_LEN).contains(&len));
        // SAFETY: the header and the table lie inside the mapping (the
        // header by the assert above, the table by `layout::max_slots`),
        // are `u64`-aligned (aligned base, `u64`-multiple offsets), and
        // consist entirely of atomics zero-initialized at creation — so
        // shared borrows for `'m` are valid even while other threads and
        // processes access the same memory through these same atomics. The
        // payload area keeps the rest of the mapping as a raw slice;
        // `layout` bounds every span carved from it.
        unsafe {
            Self {
                header: &*base.cast::<Header>(),
                table: slice::from_raw_parts(
                    base.add(layout::HEADER_LEN).cast::<AtomicU64>(),
                    layout::max_slots(len),
                ),
                payloads: std::ptr::slice_from_raw_parts_mut(
                    base.add(layout::payload_base(len)),
                    layout::payload_region_len(len),
                ),
                len,
            }
        }
    }
}

// --- The writer side: claim, fill, finish ----------------------------------

/// A concurrent shared-memory frame writer.
///
/// Safe to use across threads and processes at the same time: frames are
/// reserved with atomic operations, filled in uniquely owned payload spans,
/// and published with an atomic commit (see the ordering contract above).
pub struct ShmWriter<M> {
    mem: M,
}

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClaimError {
    /// The receiver closed the channel; anything after this point is
    /// outside the channel's boundary.
    #[error("the channel has been closed by the receiver")]
    Closed,
    /// The claim was refused for space: the region was full, or the frame
    /// was larger than the `i32::MAX`-byte frame limit. The loss is
    /// already recorded, so the channel will report itself incomplete.
    #[error("no space left in the shared-memory region")]
    Capacity,
}

impl<M: AsRawSlice> ShmWriter<M> {
    /// Creates a writer backed by a shared-memory region.
    ///
    /// # Safety
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the writer's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    ///
    /// # Panics
    ///
    /// Panics when the region is not `u64`-aligned or its size is outside the
    /// supported range (see [`SharedState::borrow`]).
    pub unsafe fn new(mem: M) -> Self {
        // Validate the region geometry eagerly so misuse fails at
        // construction, not at the first claim.
        // SAFETY: forwarded from this function's contract.
        let _ = unsafe { SharedState::borrow(mem.as_raw_slice()) };
        Self { mem }
    }

    fn state(&self) -> SharedState<'_> {
        // SAFETY: `new` requires the region to stay valid and
        // protocol-governed for the writer's lifetime, and it validated the
        // geometry.
        unsafe { SharedState::borrow(self.mem.as_raw_slice()) }
    }

    /// Whether the receiver has closed the channel.
    pub fn is_closed(&self) -> bool {
        self.state().header.claims.load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Claims a frame of exactly `frame_size` bytes.
    ///
    /// The frame is invisible to the receiver until [`FrameMut::finish`]
    /// commits it. Dropping the frame without finishing abandons the claim:
    /// the receiver ignores the slot, exactly as if the writer had died.
    /// Frames larger than `i32::MAX` bytes are refused as
    /// [`ClaimError::Capacity`].
    ///
    /// Wait-free: two `fetch_add`s, no retry loop (rule 1). A claim that
    /// does not fit fails after setting the loss flag, which is what tells
    /// the receiver a record was lost.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let state = self.state();
        let payload_len = frame_size.get();

        // Records that a claim failed and its record was lost, before the
        // writer moves on (rule 1).
        let report_loss = || {
            state.header.lost.store(1, Ordering::Relaxed);
            ClaimError::Capacity
        };

        // No descriptor can describe a payload this long; refuse it before
        // touching the counters, so the channel keeps working for every
        // record after it.
        if payload_len > layout::MAX_PAYLOAD_LEN {
            return Err(report_loss());
        }
        let reserved_len = layout::reserved_payload_len(payload_len);

        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot. A failed reservation stays counted — overshoot is harmless
        // because the counter is not what locates payloads (descriptors are)
        // and a `u64` cannot realistically wrap.
        let payload_start =
            state.header.payload_reserved.fetch_add(reserved_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(reserved_len as u64);
        if payload_end.is_none_or(|end| end > state.payloads.len() as u64) {
            return Err(report_loss());
        }
        let payload_start = usize::try_from(payload_start).expect("bounded by the payload region");

        let claims = state.header.claims.fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            // Not a loss: a record refused after close describes an
            // operation performed outside the channel's boundary.
            return Err(ClaimError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= state.table.len() {
            return Err(report_loss());
        }

        // SAFETY: the claim reserved
        // `[payload_start, payload_start + reserved_len)` — inside the
        // payload region by the capacity check above — exclusively for this
        // frame: other writers reserve disjoint spans, and the receiver
        // never reads a payload before observing its committed descriptor,
        // which `finish` publishes only when it consumes this borrow.
        let content = unsafe {
            slice::from_raw_parts_mut(state.payloads.cast::<u8>().add(payload_start), payload_len)
        };
        Ok(FrameMut {
            state,
            slot_index,
            descriptor: layout::committed(
                layout::payload_base(state.len) + payload_start,
                payload_len,
            ),
            content,
        })
    }

    // Unwrap `self` and return the underlying memory.
    #[cfg(all(test, not(miri)))]
    pub fn into_memory(self) -> M {
        self.mem
    }

    #[cfg(test)]
    pub fn try_write_frame(&self, frame: &[u8]) -> bool {
        let Some(frame_size) = NonZeroUsize::new(frame.len()) else {
            return false;
        };
        let Ok(mut frame_mut) = self.claim_frame(frame_size) else {
            return false;
        };
        frame_mut.copy_from_slice(frame);
        frame_mut.finish();
        true
    }
}

/// An exclusively owned, claimed-but-unpublished frame.
///
/// [`FrameMut::finish`] commits the frame; it is the only way to make the
/// payload visible to the receiver. Dropping the frame instead abandons
/// the claim: the slot stays unfinished and the receiver ignores it,
/// exactly as if the writer had died there. A writer that abandons a frame
/// and still performs the operation it described steps outside the usage
/// contract — records are published before the recorded operation.
pub struct FrameMut<'a> {
    state: SharedState<'a>,
    slot_index: usize,
    descriptor: u64,
    content: &'a mut [u8],
}

impl fmt::Debug for FrameMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameMut")
            .field("slot_index", &self.slot_index)
            .field("len", &self.content.len())
            .finish_non_exhaustive()
    }
}

impl Deref for FrameMut<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.content
    }
}

impl DerefMut for FrameMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.content
    }
}

impl FrameMut<'_> {
    /// Commits the frame, making it visible to the receiver.
    ///
    /// If the receiver closed the channel and aborted this frame's slot
    /// first, the swap fails and the frame is silently discarded: the
    /// record belongs to the close race and is intentionally excluded
    /// either way.
    pub fn finish(self) {
        // Rule 2: `Release` orders every payload write before the
        // descriptor.
        let _ = self.state.table[self.slot_index].compare_exchange(
            layout::UNFINISHED,
            self.descriptor,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

// --- The reader side: close and iterate -------------------------------------
//
// Closing never waits for writers, and no payload byte is read or copied:
// the reader keeps the mapping alive and hands out borrows of the validated
// committed spans on demand. Those borrows are sound because of the
// protocol, not despite it: a committed span is never written again
// (committing consumes the writer's frame), every borrow covers exactly one
// validated committed span, and everything a live writer may still touch —
// counters, slots, its own claimed or abandoned spans — is disjoint from
// every committed span. This rests on the constructor contract that the
// region is accessed only through this protocol; a process scribbling
// outside the protocol is outside the trust model.

/// Shared-memory metadata that could not have been produced by this
/// protocol. The region was corrupted; its frames are unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// A reader over the committed frames of a closed channel: validated spans
/// borrowed from the mapping, which stays alive inside this value and is
/// released when the reader drops.
pub struct ShmReader<M> {
    mem: M,
    spans: Vec<PayloadSpan>,
    complete: bool,
}

impl<M: AsRawSlice> ShmReader<M> {
    /// Closes the channel over a shared-memory region and returns the
    /// reader of its committed frames.
    ///
    /// Never blocks on writers: writers admitted before the snapshot race
    /// per slot, and each raced slot independently ends up committed
    /// (included) or aborted (excluded). Claims after the snapshot land in
    /// slots this pass never visits until the CLOSED gate — set before
    /// this returns — stops them. See the protocol docs at the top of this
    /// module.
    ///
    /// # Safety
    ///
    /// Same contract as [`ShmWriter::new`]:
    ///
    /// - `mem.as_raw_slice()` must return a stable, valid pointer to the
    ///   whole region for the reader's lifetime.
    /// - The region must have been zero-initialized when it was created and
    ///   accessed only through this protocol since.
    ///
    /// # Errors
    ///
    /// [`ProtocolError`] when the shared-memory metadata could not have
    /// been produced by a correct writer; the region was corrupted and its
    /// frames are unusable.
    ///
    /// # Panics
    ///
    /// Panics when the region is not `u64`-aligned or its size is outside
    /// the supported range (see [`SharedState::borrow`]).
    pub unsafe fn close(mem: M) -> Result<Self, ProtocolError> {
        let mut spans = Vec::new();
        let complete;
        {
            // SAFETY: forwarded from this function's contract; the raw
            // slice stays valid while `mem` is borrowed here and beyond,
            // since it moves into the returned reader.
            let state = unsafe { SharedState::borrow(mem.as_raw_slice()) };

            // The close boundary (rule 1): claims at or before this
            // snapshot are inside it, later ones land in slots this pass
            // never visits. The count is clamped to the table capacity, so
            // a counter inflated by failed claims (or by a foreign
            // scribble) degrades to a full-table sweep, not an error.
            let claims = state.header.claims.load(Ordering::Relaxed) & !CLOSED;
            let slot_count = usize::try_from(claims).unwrap_or(usize::MAX).min(state.table.len());
            // One read of the loss flag: it either sees a loss, or the loss
            // belongs to an operation performed after the boundary (rule 1).
            complete = state.header.lost.load(Ordering::Relaxed) == 0;

            // Gate further claims, so stragglers stop claiming (and
            // materializing pages) forever. Cheap: the creator pre-faulted
            // this page where first touches are expensive. Claims racing
            // between the snapshot and this gate are dropped soundly (see
            // the module docs above).
            state.header.claims.fetch_or(CLOSED, Ordering::Relaxed);

            // Freeze pass: drive every admitted slot to a terminal state
            // and collect the committed spans. After this loop the
            // snapshot's slice of the descriptor table can no longer change
            // — late writers lose their commit race against `ABORTED`.
            for slot_index in 0..slot_count {
                // Rule 3: `Acquire` on failure makes a committed payload
                // visible.
                let Err(bits) = state.table[slot_index].compare_exchange(
                    layout::UNFINISHED,
                    layout::ABORTED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) else {
                    // The receiver won the race: the unfinished slot is now
                    // aborted and stays ignored.
                    continue;
                };
                if bits == layout::ABORTED {
                    // Aborted by an earlier close over the same region;
                    // still ignored.
                    continue;
                }
                // Any other terminal value must be a committed descriptor
                // with a valid span; a foreign scribble fails the decode.
                let span = layout::decode(state.len, bits)
                    .ok_or(ProtocolError::CorruptDescriptor { slot_index })?;
                spans.push(span);
            }
        }

        Ok(Self { mem, spans, complete })
    }

    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> Iter<'_> {
        self.into_iter()
    }

    /// Whether every record a writer published made it in.
    ///
    /// False when a claim failed before the channel closed — the region
    /// was out of space, or a frame exceeded the frame limit: its record
    /// was lost, and the frames under-report what writers went on to do.
    /// Consumers that need completeness must reject them.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

impl<M> fmt::Debug for ShmReader<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShmReader")
            .field("frames", &self.spans.len())
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// Iterator over a [`ShmReader`]'s committed frames, in claim order.
pub struct Iter<'a> {
    /// Base address of the mapping the spans point into.
    base: *const u8,
    spans: slice::Iter<'a, PayloadSpan>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let span = self.spans.next()?;
        // SAFETY: `ShmReader::close` validated the span against the
        // mapping's layout, and a committed span is immutable for the
        // mapping's lifetime (see the section comment above); the reader
        // borrowed for `'a` keeps the mapping alive and mapped.
        Some(unsafe { slice::from_raw_parts(self.base.add(span.offset), span.len) })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.spans.size_hint()
    }
}

impl<'a, M: AsRawSlice> IntoIterator for &'a ShmReader<M> {
    type IntoIter = Iter<'a>;
    type Item = &'a [u8];

    fn into_iter(self) -> Iter<'a> {
        Iter { base: self.mem.as_raw_slice().cast::<u8>().cast_const(), spans: self.spans.iter() }
    }
}

/// Materializes the page backing the protocol header without changing
/// protocol state, so that neither a writer's first claim nor
/// [`ShmReader::close`]'s snapshot pays for the backing file's first
/// block allocation — a millisecond-scale cost on some journalling
/// filesystems, for reads of holes as well as writes. Run it off any
/// latency-sensitive path. Only Linux channels use this: elsewhere the
/// first touch is cheap.
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`].
#[cfg(target_os = "linux")]
pub unsafe fn pre_fault(mem: &impl AsRawSlice) {
    // SAFETY: forwarded from this function's contract.
    let state = unsafe { SharedState::borrow(mem.as_raw_slice()) };
    // A compare-exchange of zero with zero on the claim counter: on an
    // untouched region it performs a real write — allocating the first
    // block of a sparse backing file — without changing protocol state. If
    // a claim got there first, the page is already backed and the failed
    // exchange changes nothing. (An `or` of zero would not do: the
    // compiler may lower it to a plain load, which materializes only a
    // hole page without allocating the block.)
    let _ = state.header.claims.compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicU64, Ordering},
        },
        thread,
    };

    use assert2::assert;
    use bstr::BStr;

    use super::*;

    /// A mocked shared memory region for testing.
    ///
    /// To be testable for miri, the shared memory is allocated using `Arc`
    /// instead of real shared memory APIs.
    #[derive(Clone)]
    struct MockedShm {
        // Why usize: to ensure alignment
        //
        // Why not Arc<[T]>:
        // According to miri, from the perspective of data racing, incrementing
        // the ref count of Arc<[T]> is considered the same as reading the
        // content of [T], which conflicts with writing to [T] by `ShmWriter`.
        // This problem is unrelated to real shared memory.
        mem: Arc<Vec<usize>>,
        /// The actual requested byte length.
        ///
        /// Over-allocation might happen to ensure alignment of `usize`, so
        /// `mem.len()` might be inaccurate.
        len: usize,
    }
    // SAFETY: `MockedShm` uses `Arc<Vec<usize>>` for its backing memory, which
    // is safe to send across threads. The raw pointer access through
    // `AsRawSlice` is synchronized by the protocol's atomic operations.
    unsafe impl Send for MockedShm {}
    // SAFETY: Concurrent access to the shared memory is synchronized by the
    // protocol's atomic operations. The `Arc` keeps the allocation alive.
    unsafe impl Sync for MockedShm {}
    impl MockedShm {
        fn alloc(len: usize) -> Self {
            // allocates this many of usize to fit the requested byte size
            let size_in_usize = len / size_of::<usize>() + 1;

            let mem: Vec<usize> = std::iter::repeat_n(0usize, size_in_usize).collect();

            Self { mem: Arc::new(mem), len }
        }

        /// Overwrites one raw `u64` of the region, simulating foreign-process
        /// corruption of protocol metadata.
        fn poke_u64(&self, byte_offset: usize, value: u64) {
            // SAFETY: the offsets used by tests lie within the allocation and
            // are `u64`-aligned; the atomic store synchronizes with the
            // protocol's atomic accesses of the same `u64`.
            let atomic = unsafe {
                AtomicU64::from_ptr(self.as_raw_slice().cast::<u8>().add(byte_offset).cast())
            };
            atomic.store(value, Ordering::Relaxed);
        }
    }

    impl AsRawSlice for MockedShm {
        fn as_raw_slice(&self) -> *mut [u8] {
            slice_from_raw_parts_mut(Vec::as_ptr(&self.mem).cast::<u8>().cast_mut(), self.len)
        }
    }

    fn collect_frames(shm: &MockedShm) -> ShmReader<MockedShm> {
        // SAFETY: `MockedShm` provides a stable, zero-initialized allocation
        // accessed only through the protocol.
        unsafe { ShmReader::close(shm.clone()) }.unwrap()
    }

    #[test]
    fn single_thread_basic() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: `MockedShm::alloc` provides a valid, properly-sized,
        // zero-initialized allocation.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));
        assert!(writer.try_write_frame(b"world"));
        assert!(writer.try_write_frame(b"this is a test"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"hello");
        assert!(iter.next().unwrap() == b"world");
        assert!(iter.next().unwrap() == b"this is a test");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[test]
    fn zero_sized_frames_are_rejected() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));
        assert!(!writer.try_write_frame(b""));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"hello");
        assert!(iter.next() == None);
    }

    #[test]
    fn frame_spanning_many_u64s_roundtrips_exactly() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        let pattern: Vec<u8> = (0..=99).collect();
        assert!(writer.try_write_frame(&pattern));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == pattern.as_slice());
        assert!(iter.next() == None);
    }

    #[test]
    fn full_region_marks_the_channel_incomplete() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        assert!(writer.try_write_frame(b"test"));

        // Larger than the payload region: the claim fails and sets the
        // loss flag, which is what tells the receiver a record was lost.
        assert!(!writer.try_write_frame(&vec![0u8; 2048]));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"test");
        assert!(iter.next() == None);
        assert!(!frames.is_complete());
    }

    #[test]
    fn oversized_frame_is_refused_and_marks_incomplete() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        // No descriptor can describe a frame this long: the claim is
        // refused without touching the counters, so later records still
        // flow — but the loss flag marks the channel incomplete.
        let oversized = ((i32::MAX as usize) + 1).try_into().unwrap();
        assert!(matches!(writer.claim_frame(oversized), Err(ClaimError::Capacity)));
        assert!(writer.try_write_frame(b"still open"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"still open");
        assert!(iter.next() == None);
        assert!(!frames.is_complete());
    }

    #[test]
    fn crash_after_claim_is_skipped() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // A crash right after claiming and an abandoned frame leave the
        // identical state: an unfinished slot.
        let _ = writer.claim_frame(5.try_into().unwrap()).unwrap();

        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        // Death loses no performed operation, so the channel stays complete.
        assert!(frames.is_complete());
    }

    #[test]
    fn crash_during_partial_write_is_skipped() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // Simulate a crash during writing: the frame is abandoned
        // half-filled.
        {
            let mut frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
            frame[..3].copy_from_slice(b"wor");
        }

        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[test]
    fn consecutive_crashes_do_not_hide_later_frames() {
        // Two unfinished slots in a row, in both orders, must not prevent the
        // receiver from finding the valid frames around them.
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        assert!(writer.try_write_frame(b"foo"));

        // Crash after claim (slot stays zero, payload untouched).
        let _ = writer.claim_frame(5.try_into().unwrap()).unwrap();

        // Crash mid-write (slot stays zero, payload partially filled).
        {
            let mut frame = writer.claim_frame(7.try_into().unwrap()).unwrap();
            frame[..3].copy_from_slice(b"wor");
        }

        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[test]
    fn abandoned_frame_is_ignored() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // Dropping an unfinished frame abandons it: the receiver ignores
        // the slot exactly as if the writer had died there.
        let _ = writer.claim_frame(5.try_into().unwrap()).unwrap();

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pre_fault_does_not_disturb_protocol_state() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        // On the untouched region, before any claim.
        // SAFETY: see `collect_frames`.
        unsafe { pre_fault(&shm) };
        assert!(writer.try_write_frame(b"foo"));
        // Racing an already claimed region must change nothing either.
        // SAFETY: see `collect_frames`.
        unsafe { pre_fault(&shm) };
        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[test]
    fn slot_capacity_failure_marks_incomplete() {
        // A 1024-byte region has a 15-slot table; the 16th claim must fail
        // on the slot side while payload space remains.
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        for _ in 0..15 {
            assert!(writer.try_write_frame(b"x"));
        }
        assert!(writer.claim_frame(1.try_into().unwrap()).unwrap_err() == ClaimError::Capacity);

        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 15);
        assert!(!frames.is_complete());
    }

    #[test]
    fn claims_after_close_are_gated_without_poisoning() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        assert!(!writer.is_closed());
        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next() == None);
        assert!(frames.is_complete());

        // Close set the gate: a straggler's claim fails cleanly and does
        // not mark the channel incomplete — the operation is outside the
        // closed boundary.
        assert!(writer.is_closed());
        assert!(writer.claim_frame(5.try_into().unwrap()).unwrap_err() == ClaimError::Closed);
        assert!(frames.is_complete());
    }

    #[test]
    fn commit_after_abort_publishes_nothing() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        let mut frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
        frame.copy_from_slice(b"late!");

        // The receiver closes while the frame is unfinished and aborts it.
        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 0);
        assert!(frames.is_complete());

        // The late commit loses the race silently; the abandoned-frame flag
        // must not fire either, because the frame *was* explicitly finished.
        frame.finish();

        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 0);
        assert!(frames.is_complete());
    }

    #[test]
    fn concurrent() {
        // 16 KiB: a 255-slot table for the 120 frames written below.
        let shm = MockedShm::alloc(1024 * 16);

        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    // SAFETY: see `single_thread_basic`. The clone shares the
                    // same backing memory, which is safe because the protocol
                    // synchronizes concurrent access with atomics.
                    let writer = unsafe { ShmWriter::new(shm.clone()) };
                    for _ in 0..10 {
                        assert!(writer.try_write_frame(b"hello"));
                        assert!(writer.try_write_frame(b"foo"));
                        assert!(writer.try_write_frame(b"this is a test"));
                    }
                });
            }
        });

        let frames = collect_frames(&shm);
        let mut count = 0;
        for frame in &frames {
            count += 1;
            let frame = BStr::new(frame);
            assert!(frame == b"hello" || frame == b"foo" || frame == b"this is a test");
        }
        assert!(count == 120);
        assert!(frames.is_complete());
    }

    #[test]
    fn concurrent_exceeded_size() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..10 {
                        writer.try_write_frame(b"hello");
                        writer.try_write_frame(b"foo");
                        writer.try_write_frame(b"this is a test");
                    }
                });
            }
        });

        let frames = collect_frames(&shm);
        let mut count = 0;
        for frame in &frames {
            count += 1;
            let frame = BStr::new(frame);
            assert!(frame == b"hello" || frame == b"foo" || frame == b"this is a test");
        }
        // Some writes must have succeeded (the table holds 15 slots), some
        // must have failed on capacity; the failures poison completeness.
        assert!(count > 5);
        assert!(count < 120);
        assert!(!frames.is_complete());
    }

    #[test]
    fn close_races_with_active_writers() {
        let shm = MockedShm::alloc(1024 * 64);
        let barrier = Barrier::new(3);

        let (frames, results) = thread::scope(|s| {
            let writers = [(); 2].map(|()| {
                s.spawn(|| {
                    // SAFETY: see `concurrent`.
                    let writer = unsafe { ShmWriter::new(shm.clone()) };
                    barrier.wait();
                    let mut written = 0usize;
                    // Bounded so the test terminates even if close is slow;
                    // the region is large enough that capacity never fails.
                    for _ in 0..200 {
                        match writer.claim_frame(5.try_into().unwrap()) {
                            Ok(mut frame) => {
                                frame.copy_from_slice(b"hello");
                                frame.finish();
                                written += 1;
                            }
                            Err(ClaimError::Closed) => break,
                            Err(ClaimError::Capacity) => panic!("region unexpectedly full"),
                        }
                    }
                    written
                })
            });

            barrier.wait();
            let frames = collect_frames(&shm);
            let results = writers.map(|writer| writer.join().unwrap());
            (frames, results)
        });

        // Every admitted slot resolved to a whole frame or was aborted:
        // the receiver observed only complete payloads.
        let mut count = 0;
        for frame in &frames {
            count += 1;
            assert!(frame == b"hello");
        }
        // Only commits that lost the freeze race may be missing, and no
        // frame can appear that was never finished.
        let written: usize = results.into_iter().sum();
        assert!(count <= written);
        // Writers either finished or cleanly observed `Closed`; nothing was
        // abandoned, so completeness holds.
        assert!(frames.is_complete());
    }

    #[test]
    fn corrupt_committed_descriptor_is_a_protocol_error() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));

        // Point slot 0 at a span escaping the mapping.
        let bogus_len = 8u64;
        let bogus_offset = 1020u64;
        shm.poke_u64(64, (bogus_len << 32) | bogus_offset);

        // SAFETY: see `collect_frames`.
        let result = unsafe { ShmReader::close(shm) };
        assert!(result.unwrap_err() == ProtocolError::CorruptDescriptor { slot_index: 0 });
    }

    #[test]
    fn corrupt_aborted_descriptor_is_a_protocol_error() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));

        // The aborted bit combined with payload bits is a value no protocol
        // operation produces.
        shm.poke_u64(64, (1 << 63) | (8u64 << 32) | 8);

        // SAFETY: see `collect_frames`.
        let result = unsafe { ShmReader::close(shm) };
        assert!(result.unwrap_err() == ProtocolError::CorruptDescriptor { slot_index: 0 });
    }

    #[test]
    fn overshot_claim_counter_clamps_to_the_table() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));

        // A wildly inflated claim counter — mass claim failures or a foreign
        // scribble — degrades to a full-table sweep, never out-of-bounds
        // slot access: the committed frame survives, the untouched slots
        // freeze as aborted.
        shm.poke_u64(0, (1 << 40) | 1);

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"hello");
        assert!(iter.next() == None);
    }

    #[test]
    fn misaligned_region_is_rejected() {
        struct Misaligned(MockedShm);
        impl AsRawSlice for Misaligned {
            fn as_raw_slice(&self) -> *mut [u8] {
                let raw_slice = self.0.as_raw_slice();
                slice_from_raw_parts_mut(
                    // SAFETY: Adding 1 byte to create a deliberately
                    // misaligned pointer for testing. The original allocation
                    // is large enough that adding 1 byte stays within bounds.
                    unsafe { raw_slice.cast::<u8>().add(1) },
                    raw_slice.len() - 1,
                )
            }
        }

        let misaligned_shm = Misaligned(MockedShm::alloc(1024));
        assert!(misaligned_shm.as_raw_slice().cast::<u8>() as usize % align_of::<u64>() != 0);

        let result = std::panic::catch_unwind(|| {
            // SAFETY: Intentionally passing a misaligned pointer to test that
            // the geometry assertion correctly panics.
            unsafe { ShmWriter::new(misaligned_shm) };
        });
        assert!(result.is_err(), "should panic on a misaligned region");
    }

    #[test]
    #[cfg(not(miri))]
    fn real_shm_across_processes() {
        use std::process::{Child, Command};

        use rustc_hash::FxHashSet;
        use subprocess_test::command_for_fn;

        const CHILD_COUNT: usize = 12;
        const FRAME_COUNT_EACH_CHILD: usize = 100;

        const SHM_SIZE: usize = 1024 * 1024;

        let shm_path = crate::ipc::channel::shm_backing_path().unwrap();
        let shm_name = shm_path.to_str().expect("test temp dir is UTF-8").to_owned();
        let c_path = crate::ipc::channel::os_c_string(shm_path.as_os_str()).unwrap();
        let handle = fspy_shm::create(c_path.as_c_str().as_thin(), SHM_SIZE).unwrap();
        let _keeper = crate::ipc::channel::ShmKeeper { path: c_path };
        // Map before the children run. Windows keeps views coherent while they
        // exist at the same time; a view created after every writer exited can
        // observe the file before the writers' dirty pages reach it.
        let mapping = handle.map().unwrap();

        let children: Vec<Child> = (0..CHILD_COUNT)
            .map(|child_index| {
                let cmd = command_for_fn!(
                    (shm_name.clone(), child_index),
                    |(shm_name, child_index): (String, usize)| {
                        let c_path =
                            crate::ipc::channel::os_c_string(std::ffi::OsStr::new(&shm_name))
                                .unwrap();
                        let mapping =
                            fspy_shm::open(c_path.as_c_str().as_thin()).unwrap().map().unwrap();
                        // SAFETY: `mapping` is a freshly mapped shared memory
                        // region with a valid pointer and size; the protocol
                        // synchronizes concurrent access.
                        let writer = unsafe { ShmWriter::new(mapping) };
                        for i in 0..FRAME_COUNT_EACH_CHILD {
                            let frame_data = std::format!("{child_index} {i}");
                            assert!(writer.try_write_frame(frame_data.as_bytes()));
                        }
                    }
                );
                Command::from(cmd).spawn().unwrap()
            })
            .collect();

        for mut c in children {
            let status = c.wait().unwrap();
            assert!(status.success());
        }

        // SAFETY: the mapping is a valid shared-memory region created zeroed
        // and accessed only through the protocol.
        let frames = unsafe { ShmReader::close(mapping) }.unwrap();
        assert!(frames.is_complete());
        let collected = frames.iter().map(BStr::new).collect::<FxHashSet<&BStr>>();
        assert!(collected.len() == CHILD_COUNT * FRAME_COUNT_EACH_CHILD);
        for child_index in 0..CHILD_COUNT {
            for i in 0..FRAME_COUNT_EACH_CHILD {
                let frame_data = format!("{child_index} {i}");
                assert!(collected.contains(&BStr::new(frame_data.as_bytes())));
            }
        }
    }

    /// A writer killed mid-frame (SIGKILL on Unix, `TerminateProcess` on
    /// Windows — both via `Child::kill`) must not lose other writers' frames
    /// or completeness: no cleanup code runs in the killed process.
    #[test]
    #[cfg(not(miri))]
    fn killed_writer_does_not_poison_the_channel() {
        use std::{
            io::{BufRead as _, BufReader},
            process::{Command, Stdio},
        };

        use subprocess_test::command_for_fn;

        const SHM_SIZE: usize = 1024 * 1024;

        let shm_path = crate::ipc::channel::shm_backing_path().unwrap();
        let shm_name = shm_path.to_str().expect("test temp dir is UTF-8").to_owned();
        let c_path = crate::ipc::channel::os_c_string(shm_path.as_os_str()).unwrap();
        let handle = fspy_shm::create(c_path.as_c_str().as_thin(), SHM_SIZE).unwrap();
        let _keeper = crate::ipc::channel::ShmKeeper { path: c_path };
        let mapping = handle.map().unwrap();

        let cmd = command_for_fn!(shm_name, |shm_name: String| {
            let c_path = crate::ipc::channel::os_c_string(std::ffi::OsStr::new(&shm_name)).unwrap();
            let child_mapping = fspy_shm::open(c_path.as_c_str().as_thin()).unwrap().map().unwrap();
            // SAFETY: see `real_shm_across_processes`.
            let writer = unsafe { ShmWriter::new(child_mapping) };
            let mut frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
            frame[..3].copy_from_slice(b"wor");
            // Signal the parent that the frame is claimed and partially
            // written, then wait to be killed.
            #[expect(clippy::print_stdout, reason = "readiness handshake with the parent")]
            {
                println!("claimed");
            }
            let _ = frame;
            // Wait to be killed; nothing ever unparks this thread.
            loop {
                std::thread::park();
            }
        });
        let mut command = Command::from(cmd);
        command.stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        assert!(line.trim() == "claimed");
        child.kill().unwrap();
        child.wait().unwrap();

        // A surviving writer keeps working after the kill.
        // SAFETY: see `real_shm_across_processes`.
        let writer = unsafe { ShmWriter::new(mapping) };
        assert!(writer.try_write_frame(b"alive"));

        // SAFETY: see `real_shm_across_processes`.
        let frames = unsafe { ShmReader::close(writer.into_memory()) }.unwrap();
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"alive");
        assert!(iter.next() == None);
        // The killed writer left only an unfinished slot; the counters
        // stayed within their limits, so the channel is complete.
        assert!(frames.is_complete());
    }
}

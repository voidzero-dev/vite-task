//! Everything that touches shared-memory bytes, in reading order: the
//! typed views of the region, the writer side (claim, fill, finish), and
//! the receiver side (close and [`Frames`]).
//!
//! One unsafe borrow in [`SharedState::borrow`] constructs three typed
//! views of the region — the `repr(C)` [`Header`], the descriptor table as
//! a slice of atomics sized by the mapping length, and the untyped payload
//! area as a raw slice. Every access after that is a plain field access or
//! a bounds-checked index. The payload area stays raw because writers hold
//! exclusive `&mut` borrows into it, which must not alias any shared
//! reference.
//!
//! # Shared atomics
//!
//! The header holds two independent monotonic `AtomicU64` counters and a
//! loss flag:
//!
//! - the **claim counter**: bit 63 is the CLOSED gate, the low bits count
//!   claims ever attempted. Claiming is one wait-free `fetch_add`; the
//!   returned old value carries the claim's slot index, the gate, and — by
//!   comparison against the fixed table capacity — the capacity verdict.
//! - the **payload counter**: payload bytes ever reserved, bumped by another
//!   wait-free `fetch_add`.
//! - the **loss flag**: set by every failed claim before the writer moves
//!   on. The receiver derives completeness from it alone.
//!
//! Failed claims leave the counters bumped; that is harmless, because the
//! receiver clamps instead of trusting the counts, and committed
//! descriptors carry their own offset and length, so the counters never
//! locate data.
//!
//! # Memory-ordering contract
//!
//! Three synchronization rules cover the whole protocol:
//!
//! 1. **Claim versus close** — the receiver's close boundary is a plain
//!    snapshot load of the claim counter: claims ordered at or before the
//!    value it reads (in the counter's modification order) are in the
//!    snapshot; later ones receive slot indices the receiver never visits.
//!    Claims publish no payload data, so `Relaxed` suffices throughout.
//!    The CLOSED gate only stops stragglers from claiming (and allocating
//!    pages) forever; any claim admitted between the snapshot and the gate
//!    lands beyond the snapshot and is never observed. Completeness rides
//!    the same style of argument: a failed claim sets the loss flag before
//!    the writer performs the operation whose record was lost — so the
//!    receiver's one read of the flag either sees the loss, or the loss
//!    belongs to an operation performed after the boundary. A writer that
//!    dies before setting the flag never performed its operation, so
//!    nothing was actually lost.
//! 2. **Writer commit** — the slot compare-and-swap uses `Release`
//!    ([`SharedState::commit`]): every payload write happens-before the
//!    committed descriptor becomes visible.
//! 3. **Receiver observation** — the freeze compare-and-swap uses `Acquire`
//!    on failure ([`SharedState::freeze`]): observing a committed descriptor
//!    also makes the payload writes it published visible, so the borrows
//!    [`Frames`] later hands out read settled bytes.

use std::{
    fmt,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    slice,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    AsRawSlice,
    layout::{self, PayloadSpan, SlotState},
};

/// The CLOSED gate bit of the claim counter. The low 63 bits count claims,
/// so no realistic claim volume can carry into the gate.
const CLOSED: u64 = 1 << 63;

/// The region header: two protocol counters, padded so the descriptor
/// table starts off their cache line and there is room for future header
/// fields, which must start zeroed.
#[repr(C)]
struct Header {
    /// Bit 63 is the CLOSED gate; the low bits count claims ever attempted.
    claims: AtomicU64,
    /// Payload bytes ever reserved, including by failed claims.
    payload_reserved: AtomicU64,
    /// Nonzero once a claim failed: a record was lost and the channel is
    /// incomplete. Semantically a flag; a whole `u64` keeps the header a
    /// plain row of `u64` words.
    lost: AtomicU64,
    _reserved: [u64; 5],
}

const _: () = assert!(size_of::<Header>() == layout::HEADER_LEN);
const _: () = assert!(align_of::<Header>() == align_of::<AtomicU64>());

/// Why a claim was not admitted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReserveError {
    /// The receiver has closed the channel.
    Closed,
    /// The region is out of capacity.
    Capacity,
}

/// A successful reservation of one descriptor slot and one payload span.
#[derive(Debug)]
struct Reservation {
    /// Index of the reserved descriptor slot.
    slot_index: usize,
    /// Byte offset of the reserved payload span. `u64`-aligned.
    payload_offset: usize,
}

/// A borrowed view of the shared mapping with protocol-level operations:
/// the typed header, the descriptor table sized from the mapping length,
/// and the raw payload area.
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
    /// [`super::is_supported_region_len`] first.
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

    const fn mapping_len(self) -> usize {
        self.len
    }

    /// Byte offset where the payload region starts.
    const fn payload_base(self) -> usize {
        layout::payload_base(self.len)
    }

    /// Whether the CLOSED gate has been set.
    fn is_closed(self) -> bool {
        self.header.claims.load(Ordering::Relaxed) & CLOSED != 0
    }

    /// Atomically reserves one descriptor slot and one payload span.
    ///
    /// Wait-free: two `fetch_add`s, no retry loop (rule 1). A claim that
    /// does not fit fails after setting the loss flag, which is what tells
    /// the receiver a record was lost.
    fn try_claim(self, payload_len: usize) -> Result<Reservation, ReserveError> {
        // No descriptor can describe a payload this long; refuse it before
        // touching the counters, so the channel keeps working for every
        // record after it.
        if payload_len > layout::MAX_PAYLOAD_LEN {
            return Err(self.report_loss());
        }
        let reserved_len = layout::reserved_payload_len(payload_len);

        // Payload bytes first, so a payload-capacity failure does not burn a
        // slot. A failed reservation stays counted — overshoot is harmless
        // because the counter is not what locates payloads (descriptors are)
        // and a `u64` cannot realistically wrap.
        let payload_start =
            self.header.payload_reserved.fetch_add(reserved_len as u64, Ordering::Relaxed);
        // Checked: a foreign scribble of the counter must fail the claim,
        // not wrap the bound into an out-of-bounds reservation.
        let payload_end = payload_start.checked_add(reserved_len as u64);
        if payload_end.is_none_or(|end| end > self.payloads.len() as u64) {
            return Err(self.report_loss());
        }

        let claims = self.header.claims.fetch_add(1, Ordering::Relaxed);
        if claims & CLOSED != 0 {
            // Not a loss: a record refused after close describes an
            // operation performed outside the channel's boundary.
            return Err(ReserveError::Closed);
        }
        let slot_index = usize::try_from(claims).expect("claim count exceeds usize");
        if slot_index >= self.table.len() {
            return Err(self.report_loss());
        }

        Ok(Reservation {
            slot_index,
            // In bounds: `payload_start + reserved_len` fits the payload
            // region, which ends within the mapping (`layout`).
            payload_offset: self.payload_base()
                + usize::try_from(payload_start).expect("bounded by the payload region"),
        })
    }

    /// Records that a claim failed and its record was lost, before the
    /// caller moves on (rule 1). Returns the error the failed claim
    /// reports.
    fn report_loss(self) -> ReserveError {
        self.header.lost.store(1, Ordering::Relaxed);
        ReserveError::Capacity
    }

    /// Snapshots the claim counter and the loss flag; the counter load is
    /// the receiver's close boundary (rule 1). Returns the admitted slot
    /// count, clamped to the table capacity, and whether every record made
    /// it: false once any claim failed and set the flag.
    fn snapshot(self) -> (usize, bool) {
        let claims = self.header.claims.load(Ordering::Relaxed) & !CLOSED;
        let complete = self.header.lost.load(Ordering::Relaxed) == 0;
        (usize::try_from(claims).unwrap_or(usize::MAX).min(self.table.len()), complete)
    }

    /// Sets the CLOSED gate so stragglers stop claiming.
    fn close_claims(self) {
        self.header.claims.fetch_or(CLOSED, Ordering::Relaxed);
    }

    /// Forces the page backing the header (and the table's first slots) to
    /// be materialized by the operating system before anyone touches it on
    /// a latency-sensitive path.
    ///
    /// A compare-exchange of zero with zero on the claim counter: on an
    /// untouched region it performs a real write — allocating the first
    /// block of a sparse backing file, which can cost milliseconds on
    /// journalling filesystems — without changing protocol state. If a
    /// claim got there first, the page is already backed and the failed
    /// exchange changes nothing. (An `or` of zero would not do: the
    /// compiler may lower it to a plain load, which materializes only a
    /// hole page without allocating the block.)
    ///
    /// Only Linux channels use this: elsewhere the first touch is cheap.
    #[cfg(target_os = "linux")]
    fn pre_fault(self) {
        let _ = self.header.claims.compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Publishes a committed descriptor into an unfinished slot.
    ///
    /// Returns false when the receiver aborted the slot first; the payload is
    /// then permanently unreachable and the writer must not touch it again
    /// either way.
    ///
    /// # Panics
    ///
    /// Panics when `slot_index` lies outside the table; callers only pass
    /// indices of admitted reservations.
    fn commit(self, slot_index: usize, descriptor: u64) -> bool {
        // Rule 2: `Release` orders every payload write before the descriptor.
        self.table[slot_index]
            .compare_exchange(layout::UNFINISHED, descriptor, Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }

    /// Freezes one slot during close and returns its terminal value: `ABORTED`
    /// when the receiver won the race, the committed descriptor otherwise.
    ///
    /// # Panics
    ///
    /// Panics when `slot_index` lies outside the table; the receiver only
    /// passes indices below its clamped snapshot.
    fn freeze(self, slot_index: usize) -> u64 {
        // Rule 3: `Acquire` on failure makes a committed payload visible.
        match self.table[slot_index].compare_exchange(
            layout::UNFINISHED,
            layout::ABORTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => layout::ABORTED,
            Err(terminal) => terminal,
        }
    }

    /// Pointer to a reserved payload span. The caller owns the span's
    /// exclusivity argument.
    fn payload_ptr(self, offset: usize) -> *mut u8 {
        debug_assert!((self.payload_base()..=self.len).contains(&offset));
        // SAFETY: callers pass offsets of admitted reservations, which
        // `layout` keeps inside the payload area.
        unsafe { self.payloads.cast::<u8>().add(offset - self.payload_base()) }
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
        self.state().is_closed()
    }

    /// Claims a frame of exactly `frame_size` bytes.
    ///
    /// The frame is invisible to the receiver until [`FrameMut::finish`]
    /// commits it. Dropping the frame without finishing abandons the claim:
    /// the receiver ignores the slot, exactly as if the writer had died.
    /// Frames larger than `i32::MAX` bytes are refused as
    /// [`ClaimError::Capacity`].
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let state = self.state();
        let reservation = state.try_claim(frame_size.get()).map_err(|err| match err {
            ReserveError::Closed => ClaimError::Closed,
            ReserveError::Capacity => ClaimError::Capacity,
        })?;

        let content_ptr = state.payload_ptr(reservation.payload_offset);
        // SAFETY: the claim reserved
        // `[payload_offset, payload_offset + frame_size)` exclusively for
        // this frame: other writers reserve disjoint spans, and the receiver
        // never reads a payload before observing its committed descriptor —
        // which `finish` publishes only when it consumes this borrow.
        let content = unsafe { slice::from_raw_parts_mut(content_ptr, frame_size.get()) };
        Ok(FrameMut {
            state,
            slot_index: reservation.slot_index,
            descriptor: layout::committed(reservation.payload_offset, frame_size.get()),
            content,
        })
    }

    // Unwrap `self` and return the underlying memory.
    #[cfg(test)]
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
    /// first, the frame is silently discarded: the record belongs to the
    /// close race and is intentionally excluded either way.
    pub fn finish(self) {
        self.state.commit(self.slot_index, self.descriptor);
    }
}

// --- The receiver side: close and Frames ------------------------------------
//
// Closing never waits for writers, and no payload byte is read or copied:
// `Frames` keeps the mapping alive and hands out borrows of the validated
// committed spans on demand. Those borrows are sound because of the
// protocol, not despite it: a committed span is never written again
// (committing consumes the writer's frame), every borrow covers exactly one
// validated committed span, and everything a live writer may still touch —
// counters, slots, its own claimed or abandoned spans — is disjoint from
// every committed span. This rests on the constructor contract that the
// region is accessed only through this protocol; a process scribbling
// outside the protocol is outside the trust model.

/// The committed frames of a closed channel: validated spans borrowed from
/// the mapping, which stays alive inside this value. Dropping it releases
/// the mapping.
pub struct Frames<M> {
    mem: M,
    spans: Vec<PayloadSpan>,
    complete: bool,
}

impl<M: AsRawSlice> Frames<M> {
    /// Iterates over the committed frames in claim order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        let base = self.mem.as_raw_slice().cast::<u8>().cast_const();
        self.spans.iter().map(move |span| {
            // SAFETY: `close` validated the span against this mapping's
            // layout, and a committed span is immutable for the mapping's
            // lifetime (see the section comment above), so the shared borrow
            // is valid for as long as `self` lives.
            unsafe { slice::from_raw_parts(base.add(span.offset), span.len) }
        })
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

impl<M> fmt::Debug for Frames<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frames")
            .field("frames", &self.spans.len())
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

/// Shared-memory metadata that could not have been produced by this
/// protocol. The region was corrupted; its frames are unusable.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    #[error("corrupt shared-memory frame descriptor at slot {slot_index}")]
    CorruptDescriptor { slot_index: usize },
}

/// Closes the channel and returns the committed frames as borrows of the
/// mapping, which moves into the returned [`Frames`].
///
/// Never blocks on writers: writers admitted before the snapshot race per
/// slot, and each raced slot independently ends up committed (included) or
/// aborted (excluded). Claims after the snapshot land in slots this pass
/// never visits until the CLOSED gate — set before returning — stops them.
/// See the crate-level protocol docs in [`super`].
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`]: `mem` must be a stable, valid
/// pointer to the whole region, zero-initialized at creation and accessed
/// only through this protocol.
///
/// # Panics
///
/// Panics when the region is not `u64`-aligned or its size is outside the
/// supported range (see [`SharedState::borrow`]) — a broken caller, not
/// corrupt shared data, which is reported as [`ProtocolError`] instead.
pub(super) unsafe fn close<M: AsRawSlice>(mem: M) -> Result<Frames<M>, ProtocolError> {
    let spans;
    let complete;
    {
        // SAFETY: forwarded from this function's contract; the raw slice
        // stays valid while `mem` is borrowed here and beyond, since `mem`
        // moves into the returned `Frames`.
        let state = unsafe { SharedState::borrow(mem.as_raw_slice()) };

        // The close boundary: claims at or before this snapshot are inside
        // it, later ones land in slots this pass never visits. The count is
        // clamped to the table capacity, so a counter inflated by failed
        // claims (or by a foreign scribble) degrades to a full-table sweep,
        // not an error. The snapshot also reads the loss flag: its one
        // read either sees a loss, or the loss belongs to an operation
        // performed after this boundary (rule 1 in the ordering contract
        // above).
        let (slot_count, is_complete) = state.snapshot();

        // Gate further claims. Cheap: the creator pre-faulted this page
        // where first touches are expensive. Claims racing between the
        // snapshot and this gate are dropped soundly (see the module docs
        // in `super`).
        state.close_claims();

        // Freeze pass: drive every admitted slot to a terminal state and
        // collect the committed spans. After this loop the snapshot's slice
        // of the descriptor table can no longer change — late writers lose
        // their commit race against `ABORTED`.
        spans = freeze_committed_spans(state, slot_count)?;
        complete = is_complete;
    }

    Ok(Frames { mem, spans, complete })
}

fn freeze_committed_spans(
    state: SharedState<'_>,
    slot_count: usize,
) -> Result<Vec<PayloadSpan>, ProtocolError> {
    let mut spans = Vec::new();
    for slot_index in 0..slot_count {
        match layout::decode(state.freeze(slot_index)) {
            SlotState::Aborted => {}
            SlotState::Committed { payload_offset, payload_len } => {
                let span = PayloadSpan::validate(state.mapping_len(), payload_offset, payload_len)
                    .ok_or(ProtocolError::CorruptDescriptor { slot_index })?;
                spans.push(span);
            }
            // `freeze` only returns terminal values, so `Unfinished` is
            // unreachable and grouped with the corrupt case.
            SlotState::Unfinished | SlotState::Corrupt => {
                return Err(ProtocolError::CorruptDescriptor { slot_index });
            }
        }
    }
    Ok(spans)
}

/// Materializes the pages every region touch starts with — see
/// [`SharedState::pre_fault`].
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`].
#[cfg(target_os = "linux")]
pub(super) unsafe fn pre_fault(mem: &impl AsRawSlice) {
    // SAFETY: forwarded from this function's contract.
    unsafe { SharedState::borrow(mem.as_raw_slice()) }.pre_fault();
}

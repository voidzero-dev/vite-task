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
//! The header and the descriptor table form one `repr(C)` struct whose
//! table length is a compile-time constant every endpoint shares (the
//! channel specifies it); the payload area is simply the rest of the
//! mapping ([`layout`]). Attaching constructs typed views of that struct
//! — the header is two monotonic `AtomicU64` counters: claims, carrying
//! the CLOSED gate bit, and payload bytes reserved — while the payload
//! area stays untyped bytes.
//! A claim is two wait-free `fetch_add`s — one reserves payload bytes, one
//! reserves a descriptor slot — validated against the fixed region bounds
//! from the returned old values. A failed claim sets the CLOSED gate as
//! its loss report and leaves the counters bumped, harmlessly: readers
//! clamp to the region capacities, and committed descriptors are
//! self-describing ([`layout`]), so the counters never locate data. Every
//! slot has a fixed location, so an unfinished frame can never hide a
//! later one.
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
//! # Seal boundary
//!
//! [`ShmReader::seal`]'s boundary is a snapshot of the claim counter.
//! A writer admitted before the snapshot races the freeze pass per slot and
//! its frame is either included (commit won) or ignored (abort won) — never
//! torn; a claim after the snapshot lands in a slot the receiver never
//! visits and is dropped, and the CLOSED gate set before the seal returns
//! stops stragglers from claiming (and materializing pages) forever. Both
//! drops are sound because writers publish a record *before* performing the
//! recorded operation: a process that died mid-frame never performed the
//! operation, and one that claimed or committed after the snapshot performs
//! it outside the channel's boundary. A record refused *before* the seal —
//! a full region, an oversized frame — sets the CLOSED gate first, so the
//! channel reports itself incomplete ([`ShmReader::is_complete`]) and
//! refuses every later claim: once one record is lost the receiver must
//! reject the result, and further records would be wasted work.
//!
//! Correctness never depends on writer-side cleanup: no exit hooks, PID
//! checks, heartbeats, or timeouts.

mod layout;
mod reader;
mod writer;

use std::ptr::slice_from_raw_parts_mut;
#[cfg(target_os = "linux")]
use std::sync::atomic::Ordering;

use fspy_shm::Mapping;
#[cfg(target_os = "linux")]
use layout::MappedLayout;
// Only tests name the error types; production matches on `Ok`/`Err` alone.
#[cfg(test)]
pub use reader::ProtocolError;
pub use reader::ShmReader;
#[cfg(test)]
pub use writer::ClaimError;
pub use writer::ShmWriter;

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

/// Whether a mapping of `len` bytes can host a channel with `SLOTS`
/// descriptor slots: the fixed-location struct must fit inside the
/// mapping, and the payload area — the rest — must fit the descriptors'
/// 32-bit offsets.
///
/// Senders opening a file they do not control should refuse unsupported
/// lengths with an error; the protocol's own constructors treat them as a
/// broken caller and panic. Creators size the file with [`region_len`].
#[must_use]
pub const fn is_supported_region_len<const SLOTS: usize>(len: usize) -> bool {
    size_of::<layout::Meta<SLOTS>>() <= len
        && len - size_of::<layout::Meta<SLOTS>>() <= layout::MAX_PAYLOAD_REGION_LEN
}

/// The region length for a channel with `SLOTS` descriptor slots and up
/// to `payload_capacity` payload bytes (clamped to the payload area's
/// 32-bit-offset maximum).
#[must_use]
pub const fn region_len<const SLOTS: usize>(payload_capacity: usize) -> usize {
    let payload = if payload_capacity < layout::MAX_PAYLOAD_REGION_LEN {
        payload_capacity
    } else {
        layout::MAX_PAYLOAD_REGION_LEN
    };
    size_of::<layout::Meta<SLOTS>>() + payload
}

/// Materializes the page backing the protocol header without changing
/// protocol state, so that neither a writer's first claim nor
/// [`ShmReader::seal`]'s snapshot pays for the backing file's first
/// block allocation — a millisecond-scale cost on some journalling
/// filesystems, for reads of holes as well as writes. Run it off any
/// latency-sensitive path. Only Linux channels use this: elsewhere the
/// first touch is cheap.
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`].
#[cfg(target_os = "linux")]
pub unsafe fn pre_fault<const SLOTS: usize>(mem: &impl AsRawSlice) {
    // SAFETY: forwarded from this function's contract.
    let mapped = unsafe { MappedLayout::<SLOTS>::new(mem.as_raw_slice()) };
    // A compare-exchange of zero with zero on the claim counter: on an
    // untouched region it performs a real write — allocating the first
    // block of a sparse backing file — without changing protocol state. If
    // a claim got there first, the page is already backed and the failed
    // exchange changes nothing. (An `or` of zero would not do: the
    // compiler may lower it to a plain load, which materializes only a
    // hole page without allocating the block.)
    let _ = mapped.header().claims.compare_exchange(0, 0, Ordering::Relaxed, Ordering::Relaxed);
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

    /// The table length most tests use; regions add payload room on top.
    const S: usize = 15;

    fn collect_frames(shm: &MockedShm) -> ShmReader<MockedShm, S> {
        // SAFETY: `MockedShm` provides a stable, zero-initialized allocation
        // accessed only through the protocol.
        unsafe { ShmReader::seal(shm.clone()) }.unwrap()
    }

    #[test]
    fn single_thread_basic() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: `MockedShm::alloc` provides a valid, properly-sized,
        // zero-initialized allocation.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };

        assert!(writer.try_write_frame(b"test"));

        // Larger than the payload region: the claim fails and sets the
        // gate, which is what tells the receiver a record was lost.
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"kept"));

        // No descriptor can describe a frame this long: the claim is
        // refused and sets the gate, condemning later claims — their
        // records would ride a result the receiver must already reject.
        let oversized = ((i32::MAX as usize) + 1).try_into().unwrap();
        assert!(matches!(writer.claim_frame(oversized), Err(ClaimError::Capacity)));
        assert!(writer.is_closed());
        assert!(!writer.try_write_frame(b"refused"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"kept");
        assert!(iter.next() == None);
        assert!(!frames.is_complete());
    }

    #[test]
    fn crash_after_claim_is_skipped() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };

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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };

        // On the untouched region, before any claim.
        // SAFETY: see `collect_frames`.
        unsafe { pre_fault::<S>(&shm) };
        assert!(writer.try_write_frame(b"foo"));
        // Racing an already claimed region must change nothing either.
        // SAFETY: see `collect_frames`.
        unsafe { pre_fault::<S>(&shm) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
        for _ in 0..15 {
            assert!(writer.try_write_frame(b"x"));
        }
        assert!(writer.claim_frame(1.try_into().unwrap()).unwrap_err() == ClaimError::Capacity);

        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 15);
        assert!(!frames.is_complete());
    }

    #[test]
    fn claims_after_seal_are_gated_without_poisoning() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        assert!(!writer.is_closed());
        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next() == None);
        assert!(frames.is_complete());

        // The seal set the gate: a straggler's claim fails cleanly and
        // does not mark the channel incomplete — the operation is outside
        // the sealed boundary.
        assert!(writer.is_closed());
        assert!(writer.claim_frame(5.try_into().unwrap()).unwrap_err() == ClaimError::Closed);
        assert!(frames.is_complete());
    }

    #[test]
    fn commit_after_abort_publishes_nothing() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };

        let mut frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
        frame.copy_from_slice(b"late!");

        // The receiver closes while the frame is unfinished and aborts it.
        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 0);
        assert!(frames.is_complete());

        // The late commit loses the race silently.
        frame.finish();

        // A second seal still reads no frames — and reports incomplete:
        // the gate was set by the first seal, and a re-seal cannot vouch
        // for records refused since then.
        let frames = collect_frames(&shm);
        assert!(frames.iter().count() == 0);
        assert!(!frames.is_complete());
    }

    #[test]
    fn concurrent() {
        // A 255-slot table for the 120 frames written below, with 16 KiB
        // of room for the fixed struct and the payloads.
        const S: usize = 255;
        let shm = MockedShm::alloc(1024 * 16);

        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    // SAFETY: see `single_thread_basic`. The clone shares the
                    // same backing memory, which is safe because the protocol
                    // synchronizes concurrent access with atomics.
                    let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
                    for _ in 0..10 {
                        assert!(writer.try_write_frame(b"hello"));
                        assert!(writer.try_write_frame(b"foo"));
                        assert!(writer.try_write_frame(b"this is a test"));
                    }
                });
            }
        });

        // SAFETY: see `collect_frames`.
        let frames = unsafe { ShmReader::<_, S>::seal(shm) }.unwrap();
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
    fn seal_races_with_active_writers() {
        // Plenty of slots and payload room: capacity must never fail here.
        const S: usize = 1023;
        let shm = MockedShm::alloc(1024 * 64);
        let barrier = Barrier::new(3);

        let (frames, results) = thread::scope(|s| {
            let writers = [(); 2].map(|()| {
                s.spawn(|| {
                    // SAFETY: see `concurrent`.
                    let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
                    barrier.wait();
                    let mut written = 0usize;
                    // Bounded so the test terminates even if the seal is slow;
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
            // SAFETY: see `collect_frames`.
            let frames = unsafe { ShmReader::<_, S>::seal(shm.clone()) }.unwrap();
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));

        // Point slot 0 at a span escaping the payload region.
        let bogus_len = 8u64;
        let bogus_offset = 1020u64;
        shm.poke_u64(64, (bogus_len << 32) | bogus_offset);

        // SAFETY: see `collect_frames`.
        let result = unsafe { ShmReader::<_, S>::seal(shm) };
        assert!(result.unwrap_err() == ProtocolError::CorruptDescriptor { slot_index: 0 });
    }

    #[test]
    fn corrupt_aborted_descriptor_is_a_protocol_error() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"hello"));

        // The aborted bit combined with payload bits is a value no protocol
        // operation produces.
        shm.poke_u64(64, (1 << 63) | (8u64 << 32) | 8);

        // SAFETY: see `collect_frames`.
        let result = unsafe { ShmReader::<_, S>::seal(shm) };
        assert!(result.unwrap_err() == ProtocolError::CorruptDescriptor { slot_index: 0 });
    }

    #[test]
    fn overshot_claim_counter_clamps_to_the_table() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(shm.clone()) };
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
            unsafe { ShmWriter::<_, S>::new(misaligned_shm) };
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

        // Room for every child's frames, in slots and in payload bytes.
        const S: usize = 16383;
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
                        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(mapping) };
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
        let frames = unsafe { ShmReader::<_, S>::seal(mapping) }.unwrap();
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
            let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(child_mapping) };
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
        let writer: ShmWriter<_, S> = unsafe { ShmWriter::new(mapping) };
        assert!(writer.try_write_frame(b"alive"));

        // SAFETY: see `real_shm_across_processes`.
        let frames = unsafe { ShmReader::<_, S>::seal(writer.into_memory()) }.unwrap();
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"alive");
        assert!(iter.next() == None);
        // The killed writer left only an unfinished slot; the counters
        // stayed within their limits, so the channel is complete.
        assert!(frames.is_complete());
    }
}

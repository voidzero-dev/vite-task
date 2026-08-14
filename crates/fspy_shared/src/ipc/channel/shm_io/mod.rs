//! Crash-tolerant, nonblocking frame publication in a shared memory region.
//!
//! Multiple writer processes append variable-length frames concurrently; one
//! receiver closes the channel and collects every committed frame without
//! waiting for any writer. A process may die at any instruction — mid-claim,
//! mid-write, pre-commit — and only its own unfinished frame is lost.
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
//! The header holds three monotonic words ([`state`]): a claim counter
//! carrying the CLOSED gate bit, a payload counter, and an incomplete flag.
//! A claim is two wait-free `fetch_add`s — one reserves payload bytes, one
//! reserves a descriptor slot — validated against the fixed region bounds
//! from the returned old values. Failed claims overshoot the counters
//! harmlessly: readers clamp to the region capacities, and committed
//! descriptors are self-describing ([`slot`]), so the counters never locate
//! data. Every slot has a fixed location, so an unfinished frame can never
//! hide a later one.
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
//! ([`state`]'s ordering contract). The receiver never derives frame
//! locations from payload bytes and never references memory a live writer
//! may still mutate — committed payloads are copied out with atomic loads.
//!
//! # Close boundary
//!
//! [`close`]'s boundary is a snapshot of the claim counter. A writer
//! admitted before the snapshot races the freeze pass per slot and its
//! frame is either included (commit won) or ignored (abort won) — never
//! torn; a claim after the snapshot lands in a slot the receiver never
//! visits and is dropped. Both drops are sound because writers publish a
//! record *before* performing the recorded operation: a process that died
//! mid-frame never performed the operation, and one that claimed or
//! committed after the snapshot performs it outside the run's tracking
//! boundary. A live writer that loses a record *before* close (capacity,
//! abandonment) flags the trace incomplete ([`Frames::is_complete`]).
//!
//! [`close_claims`] sets the CLOSED gate afterwards, off the
//! latency-sensitive path, so straggler processes eventually stop claiming
//! (and stop materializing pages of the region) rather than writing into
//! the void forever.
//!
//! Correctness never depends on writer-side cleanup: no exit hooks, PID
//! checks, heartbeats, or timeouts.

mod layout;
mod reader;
mod slot;
mod state;
mod writer;

use std::ptr::slice_from_raw_parts_mut;

use fspy_shm::Mapping;
pub use reader::{Frames, ProtocolError};
pub use writer::{ClaimError, FrameMut, ShmWriter, WriteEncodedError};

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

/// Closes the channel and collects the committed frames without waiting for
/// writers. See [`reader::close`].
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`]: the region must be stable and valid,
/// zero-initialized at creation, and accessed only through this protocol.
pub unsafe fn close(mem: &impl AsRawSlice) -> Result<Frames, ProtocolError> {
    // SAFETY: forwarded from this function's contract.
    unsafe { reader::close(mem.as_raw_slice()) }
}

/// The byte ranges of a `mapping_len`-byte region whose backing blocks the
/// creator should preallocate: the header page (counters plus the first
/// slots) and the start of the payload region. Every trace writes both
/// areas first; on filesystems where materializing a new area of the sparse
/// backing file costs milliseconds, reserving one block per area up front
/// keeps that cost out of a writer's first record and out of [`close`] —
/// growing an already-materialized area is cheap.
#[cfg(target_os = "linux")]
pub fn preallocation_spans(mapping_len: usize) -> impl Iterator<Item = (u64, u64)> {
    // One filesystem block suffices to start an area; clamp inside the
    // mapping so tiny (test) regions never reserve past their end.
    const SPAN_LEN: usize = 4096;
    [0, layout::payload_base(mapping_len)]
        .into_iter()
        .filter(move |&offset| offset < mapping_len)
        .map(move |offset| (offset as u64, SPAN_LEN.min(mapping_len - offset) as u64))
}

/// Sets the CLOSED gate so writers stop claiming once they observe it.
///
/// Deliberately separate from [`close`]: the boundary is [`close`]'s
/// snapshot, and this write — which materializes the counter page, a
/// millisecond-scale first-block allocation on some journalling
/// filesystems when the trace is empty — belongs off the latency-sensitive
/// path. Claims landing between the snapshot and the gate are dropped
/// soundly (see the module docs).
///
/// # Safety
///
/// Same contract as [`ShmWriter::new`].
pub unsafe fn close_claims(mem: &impl AsRawSlice) {
    // SAFETY: forwarded from this function's contract.
    unsafe { reader::close_claims(mem.as_raw_slice()) }
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

        /// Overwrites a raw word of the region, simulating foreign-process
        /// corruption of protocol metadata.
        fn poke_word(&self, byte_offset: usize, value: u64) {
            // SAFETY: the offsets used by tests lie within the allocation and
            // are word-aligned; the atomic store synchronizes with the
            // protocol's atomic accesses of the same word.
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

    fn collect_frames(shm: &MockedShm) -> Frames {
        // SAFETY: `MockedShm` provides a stable, zero-initialized allocation
        // accessed only through the protocol.
        unsafe { close(shm) }.unwrap()
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
    fn multi_word_frame_roundtrips_exactly() {
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
    fn oversized_frame_fails_and_marks_incomplete() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };

        assert!(writer.try_write_frame(b"test"));

        // Larger than the payload region, and larger than the absolute frame
        // limit: both fail the claim. The failed reservation stays counted —
        // harmless, because the failure already made the trace incomplete
        // and therefore uncacheable.
        assert!(!writer.try_write_frame(&vec![0u8; 2048]));
        assert!(
            writer.claim_frame(((i32::MAX as usize) + 1).try_into().unwrap()).unwrap_err()
                == ClaimError::Capacity
        );

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"test");
        assert!(iter.next() == None);
        // The failed claims lost records while their process lived on.
        assert!(!frames.is_complete());
    }

    #[test]
    fn crash_after_claim_is_skipped() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // Simulate a crash right after claiming: no drop code runs.
        let frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
        std::mem::forget(frame);

        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        // Death loses no performed operation, so the trace stays complete.
        assert!(frames.is_complete());
    }

    #[test]
    fn crash_during_partial_write_is_skipped() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // Simulate a crash during writing.
        let mut frame = writer.claim_frame(5.try_into().unwrap()).unwrap();
        frame[..3].copy_from_slice(b"wor");
        std::mem::forget(frame);

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
        std::mem::forget(writer.claim_frame(5.try_into().unwrap()).unwrap());

        // Crash mid-write (slot stays zero, payload partially filled).
        let mut frame = writer.claim_frame(7.try_into().unwrap()).unwrap();
        frame[..3].copy_from_slice(b"wor");
        std::mem::forget(frame);

        assert!(writer.try_write_frame(b"bar"));

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next().unwrap() == b"bar");
        assert!(iter.next() == None);
        assert!(frames.is_complete());
    }

    #[test]
    fn abandoned_frame_marks_the_trace_incomplete() {
        let shm = MockedShm::alloc(1024);
        // SAFETY: see `single_thread_basic`.
        let writer = unsafe { ShmWriter::new(shm.clone()) };
        assert!(writer.try_write_frame(b"foo"));

        // A live writer dropping an unfinished frame abandons a record it
        // may still act on.
        drop(writer.claim_frame(5.try_into().unwrap()).unwrap());

        let frames = collect_frames(&shm);
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"foo");
        assert!(iter.next() == None);
        assert!(!frames.is_complete());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn preallocation_spans_cover_both_first_touch_areas() {
        // The production mapping: one block at the header, one at the start
        // of the payload region, both inside the mapping.
        let spans: Vec<_> = preallocation_spans(layout::MAX_MAPPING_LEN).collect();
        assert!(
            spans == vec![(0, 4096), (layout::payload_base(layout::MAX_MAPPING_LEN) as u64, 4096)]
        );

        // A tiny region: spans clamp to the mapping instead of reserving
        // past its end (which would desynchronize mapping sizes across
        // processes if the reservation grew the file).
        for (offset, len) in preallocation_spans(1024) {
            assert!(offset + len <= 1024);
            assert!(len > 0);
        }
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
    fn claims_between_snapshot_and_gate_are_dropped() {
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

        // The gate is not set yet: a straggler's claim is admitted, lands
        // beyond the snapshot, and commits into a slot the receiver never
        // read — the record is dropped, not torn, and not incomplete.
        let mut late = writer.claim_frame(5.try_into().unwrap()).unwrap();
        late.copy_from_slice(b"late!");
        late.finish();
        assert!(frames.iter().count() == 1);

        // Once the gate lands, claims fail cleanly and still do not mark
        // the trace incomplete: the access is outside the closed boundary.
        // SAFETY: see `collect_frames`.
        unsafe { close_claims(&shm) };
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
        for frame in frames.iter() {
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
        for frame in frames.iter() {
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
        for frame in frames.iter() {
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
        shm.poke_word(64, (bogus_len << 32) | bogus_offset);

        // SAFETY: see `collect_frames`.
        let result = unsafe { close(&shm) };
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
        shm.poke_word(64, (1 << 63) | (8u64 << 32) | 8);

        // SAFETY: see `collect_frames`.
        let result = unsafe { close(&shm) };
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
        shm.poke_word(0, (1 << 40) | 1);

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
        let frames = unsafe { close(&mapping) }.unwrap();
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
            std::mem::forget(frame);
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
        let frames = unsafe { close(&writer.into_memory()) }.unwrap();
        let mut iter = frames.iter();
        assert!(iter.next().unwrap() == b"alive");
        assert!(iter.next() == None);
        // Death runs no drop code, so the killed writer's lost frame does not
        // mark the trace incomplete.
        assert!(frames.is_complete());
    }
}

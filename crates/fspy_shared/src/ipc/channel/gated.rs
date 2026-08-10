//! The safe surface over a shared memory region: a close gate in front of a
//! frame arena.
//!
//! Region layout:
//!
//! ```text
//! | gate word: AtomicU64 | padding to 64 bytes | frame arena |
//! ```
//!
//! The 64-byte header keeps the gate word and the arena's end-offset word on
//! separate cache lines and preserves the arena's alignment requirements. The
//! gate itself knows nothing about this layout, and neither does the channel
//! layer above: everything in between lives here.
//!
//! Writers reach the arena only through [`GatedShmWriter`], which holds a gate
//! guard for the lifetime of every frame. The read end starts as a
//! [`GatedShmReceiver`], and the only way to obtain a [`GatedShmReader`] is a
//! successful [`GatedShmReceiver::close`], so "never read memory that is still
//! being written" is a property of the types rather than of a convention.

use std::{
    fmt,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    ptr::slice_from_raw_parts_mut,
    slice,
    sync::{Arc, atomic::AtomicU64},
};

use wincode::{SchemaWrite, config::DefaultConfig};

use super::{
    gate::{EnterError, Gate, GateGuard},
    shm_io::{self, AsRawSlice, ShmReader, ShmWriter},
};

/// Bytes reserved in front of the arena for the gate word.
const GATE_REGION_SIZE: usize = 64;

/// Why a frame could not be claimed.
#[derive(thiserror::Error, Debug)]
pub enum ClaimError {
    /// The gate is closed. The caller must discard the access silently; this is
    /// the by-design outcome for a process that writes after the traced root
    /// process has exited.
    #[error("the shared memory is closed for writing")]
    Closed,
    /// The arena is full, or the gate count saturated. Both are
    /// impossible-in-practice states that callers treat as fatal.
    #[error("the shared memory has no room for another frame")]
    Capacity,
}

impl From<EnterError> for ClaimError {
    fn from(error: EnterError) -> Self {
        match error {
            EnterError::Closed => Self::Closed,
            EnterError::Saturated => Self::Capacity,
        }
    }
}

/// Why an encoded value could not be appended.
#[derive(thiserror::Error, Debug)]
pub enum WriteEncodedError {
    /// The frame could not be claimed. [`ClaimError::Closed`] is the silent-drop
    /// case.
    #[error(transparent)]
    Claim(#[from] ClaimError),
    /// The value could not be encoded into the claimed frame.
    #[error("failed to encode a value into shared memory")]
    Encode(#[from] wincode::error::WriteError),
    /// The value encodes to nothing, which the frame protocol cannot represent.
    #[error("tried to write a zero-sized frame into shared memory")]
    ZeroSizedFrame,
}

/// A close found writers still in flight, so the region cannot be read.
#[derive(thiserror::Error, Debug)]
#[error("{in_flight} traced write(s) were still in flight when the channel closed")]
pub struct StillWriting {
    /// How many gate guards were held at the instant of the close.
    pub in_flight: u64,
}

/// The write end of a gated region.
pub struct GatedShmWriter<M: AsRawSlice> {
    region: Arc<M>,
    arena: ShmWriter<Arena<M>>,
}

impl<M: AsRawSlice> GatedShmWriter<M> {
    /// Creates the write end.
    ///
    /// # Safety
    ///
    /// These are the rules nothing but the caller can check:
    ///
    /// - `mem.as_raw_slice()` returns a stable, valid, 8-byte-aligned region of
    ///   at least `GATE_REGION_SIZE` plus the arena's own minimum, for as long
    ///   as this writer lives;
    /// - the region was zero-initialized before any access;
    /// - across *all* processes, the region is accessed exclusively through
    ///   [`GatedShmWriter`] and [`GatedShmReceiver`]/[`GatedShmReader`].
    ///
    /// # Panics
    ///
    /// Panics if the region is too small or misaligned for a gate word.
    pub unsafe fn new(mem: M) -> Self {
        assert_gated_region(&mem);
        let region = Arc::new(mem);
        // SAFETY: `Arena` addresses the region past its gate header, which the
        // caller's contract makes valid, stable and zero-initialized, and which
        // nothing outside this protocol touches.
        let arena = unsafe { ShmWriter::new(Arena { region: Arc::clone(&region) }) };
        Self { region, arena }
    }

    /// Claims a frame of `size` bytes, holding the gate open until it is
    /// dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ClaimError::Closed`] once the read end has closed the gate, and
    /// [`ClaimError::Capacity`] when the arena cannot fit the frame.
    pub fn claim_frame(&self, size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let gate = self.gate().enter()?;
        let frame = self.arena.claim_frame(size).ok_or(ClaimError::Capacity)?;
        Ok(FrameMut { frame, _gate: gate })
    }

    /// Appends an encoded value as a single frame.
    ///
    /// # Errors
    ///
    /// Returns [`ClaimError::Closed`] wrapped in [`WriteEncodedError::Claim`]
    /// once the read end has closed the gate, and otherwise whatever the arena
    /// reports.
    pub fn write_encoded<T: SchemaWrite<DefaultConfig, Src = T>>(
        &self,
        value: &T,
    ) -> Result<(), WriteEncodedError> {
        // The guard is held across the inner write and released only afterwards,
        // so the frame is complete before the gate count drops.
        let _gate = self.gate().enter().map_err(ClaimError::from)?;
        self.arena.write_encoded(value).map_err(|error| match error {
            shm_io::WriteEncodedError::EncodeError(error) => WriteEncodedError::Encode(error),
            shm_io::WriteEncodedError::ZeroSizedFrame => WriteEncodedError::ZeroSizedFrame,
            shm_io::WriteEncodedError::InsufficientSpace => ClaimError::Capacity.into(),
        })
    }

    fn gate(&self) -> Gate<'_> {
        gate_of(self.region.as_ref())
    }
}

/// An exclusively owned frame together with the gate guard that keeps the region
/// open while it is written.
///
/// The frame is declared first so that its completion lands before the gate is
/// released.
pub struct FrameMut<'a> {
    frame: shm_io::FrameMut<'a>,
    _gate: GateGuard<'a>,
}

impl Deref for FrameMut<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl DerefMut for FrameMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

/// The read end before the region has been frozen.
///
/// Built once, right after the mapping is created, while it is still
/// zero-initialized and no other process knows about it.
pub struct GatedShmReceiver<M: AsRawSlice> {
    region: M,
}

impl<M: AsRawSlice> GatedShmReceiver<M> {
    /// Creates the read end.
    ///
    /// # Safety
    ///
    /// Same rules as [`GatedShmWriter::new`].
    ///
    /// # Panics
    ///
    /// Panics if the region is too small or misaligned for a gate word.
    pub unsafe fn new(mem: M) -> Self {
        assert_gated_region(&mem);
        Self { region: mem }
    }

    /// Closes the gate and, if nothing was in flight, hands out the reader.
    ///
    /// This is a single atomic operation and never blocks. On success the
    /// region is never written again: every later claim fails against the
    /// closed bit, the constructor's rules keep writers outside this protocol
    /// away, and the release/acquire pairing makes every past write visible.
    /// That is what makes borrowing it as `&[u8]` sound.
    ///
    /// # Errors
    ///
    /// Returns [`StillWriting`] when a guard was held at the instant of the
    /// close. No reader exists in that case, by construction.
    pub fn close(self) -> Result<GatedShmReader<M>, StillWriting> {
        let in_flight = gate_of(&self.region).close();
        if in_flight == 0 {
            Ok(GatedShmReader { frames: ShmReader::new(FrozenArena { region: self.region }) })
        } else {
            Err(StillWriting { in_flight })
        }
    }
}

/// A frozen region, obtainable only from a successful [`GatedShmReceiver::close`].
pub struct GatedShmReader<M: AsRawSlice> {
    frames: ShmReader<FrozenArena<M>>,
}

impl<M: AsRawSlice> GatedShmReader<M> {
    /// Iterates over every frame written before the close.
    pub fn iter_frames(&self) -> impl Iterator<Item = &[u8]> {
        self.frames.iter_frames()
    }
}

impl<M: AsRawSlice> fmt::Debug for GatedShmReader<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("GatedShmReader").finish_non_exhaustive()
    }
}

/// Addresses the arena that follows a region's gate header.
struct Arena<M> {
    region: Arc<M>,
}

impl<M: AsRawSlice> AsRawSlice for Arena<M> {
    fn as_raw_slice(&self) -> *mut [u8] {
        arena_of(self.region.as_raw_slice())
    }
}

/// Addresses the arena of a region that is known to be frozen.
struct FrozenArena<M> {
    region: M,
}

impl<M: AsRawSlice> AsRef<[u8]> for FrozenArena<M> {
    fn as_ref(&self) -> &[u8] {
        let arena = arena_of(self.region.as_raw_slice());
        // SAFETY: this type only exists after `GatedShmReceiver::close` reported
        // nothing in flight, so the region is frozen for the whole lifetime of
        // this value, and the constructor's rules keep it valid and free of
        // writers outside the protocol.
        unsafe { slice::from_raw_parts(arena.cast::<u8>(), arena.len()) }
    }
}

/// Borrows a region's gate word.
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the constructors assert the region is aligned for the gate word"
)]
fn gate_of<M: AsRawSlice>(region: &M) -> Gate<'_> {
    let base = region.as_raw_slice().cast::<u64>();
    // SAFETY: the constructors checked the region's size and 8-byte alignment,
    // the region outlives this borrow, and the caller's rules guarantee the word
    // started at zero and is only ever used as this gate.
    Gate::new(unsafe { AtomicU64::from_ptr(base) })
}

/// Splits the arena off a region's gate header.
fn arena_of(region: *mut [u8]) -> *mut [u8] {
    let len = region.len();
    debug_assert!(len > GATE_REGION_SIZE, "gated region is smaller than its gate header");
    // SAFETY: the constructors checked that the region is longer than its gate
    // header, so this stays inside the same allocation.
    let arena = unsafe { region.cast::<u8>().add(GATE_REGION_SIZE) };
    slice_from_raw_parts_mut(arena, len - GATE_REGION_SIZE)
}

/// Checks the parts of the constructors' contract that are cheap to check.
fn assert_gated_region<M: AsRawSlice>(mem: &M) {
    let region = mem.as_raw_slice();
    assert!(
        region.len() > GATE_REGION_SIZE,
        "a gated region must be larger than its {GATE_REGION_SIZE}-byte gate header"
    );
    assert_eq!(
        region.cast::<u8>() as usize % align_of::<u64>(),
        0,
        "a gated region must be aligned for its gate word"
    );
}

#[cfg(test)]
mod tests {
    use std::{str::from_utf8, sync::Arc, thread};

    use super::*;

    /// A plain heap region standing in for shared memory, so these tests also
    /// run under miri.
    #[derive(Clone)]
    struct MockRegion {
        // `u64` elements give the region the alignment a gate word needs.
        mem: Arc<Vec<u64>>,
        len: usize,
    }

    // SAFETY: the backing allocation is shared through an `Arc`, and every access
    // goes through the gated protocol, which is what makes concurrent use sound.
    unsafe impl Send for MockRegion {}
    // SAFETY: as above.
    unsafe impl Sync for MockRegion {}

    impl MockRegion {
        fn alloc(len: usize) -> Self {
            Self { mem: Arc::new(vec![0u64; len / size_of::<u64>() + 1]), len }
        }
    }

    impl AsRawSlice for MockRegion {
        fn as_raw_slice(&self) -> *mut [u8] {
            slice_from_raw_parts_mut(Vec::as_ptr(&self.mem).cast::<u8>().cast_mut(), self.len)
        }
    }

    fn endpoints(len: usize) -> (GatedShmWriter<MockRegion>, GatedShmReceiver<MockRegion>) {
        let region = MockRegion::alloc(len);
        // SAFETY: `MockRegion` is a valid, stable, 8-byte-aligned,
        // zero-initialized allocation used only through these two types.
        unsafe { (GatedShmWriter::new(region.clone()), GatedShmReceiver::new(region)) }
    }

    fn write_frame(writer: &GatedShmWriter<MockRegion>, bytes: &[u8]) -> Result<(), ClaimError> {
        let mut frame = writer.claim_frame(NonZeroUsize::new(bytes.len()).unwrap())?;
        frame.copy_from_slice(bytes);
        Ok(())
    }

    #[test]
    fn write_close_read_roundtrip() {
        let (writer, receiver) = endpoints(1024);

        write_frame(&writer, b"hello").unwrap();
        write_frame(&writer, b"world").unwrap();

        let reader = receiver.close().unwrap();
        let frames: Vec<&[u8]> = reader.iter_frames().collect();
        assert_eq!(frames, vec![b"hello".as_slice(), b"world".as_slice()]);
    }

    #[test]
    fn close_with_a_live_frame_reports_still_writing() {
        let (writer, receiver) = endpoints(1024);

        let frame = writer.claim_frame(NonZeroUsize::new(4).unwrap()).unwrap();
        // Reading anyway is not expressible: `close` consumes the receiver and
        // only hands back a reader on success, so this error value is the end of
        // the road for the read side.
        let error = receiver.close().unwrap_err();

        assert_eq!(error.in_flight, 1);
        drop(frame);
    }

    #[test]
    fn claims_after_close_are_refused() {
        let (writer, receiver) = endpoints(1024);

        write_frame(&writer, b"before").unwrap();
        let reader = receiver.close().unwrap();

        assert!(matches!(write_frame(&writer, b"after"), Err(ClaimError::Closed)));
        assert!(matches!(
            writer.write_encoded(&7u32),
            Err(WriteEncodedError::Claim(ClaimError::Closed))
        ));

        let frames: Vec<&[u8]> = reader.iter_frames().collect();
        assert_eq!(frames, vec![b"before".as_slice()]);
    }

    #[test]
    fn a_full_arena_reports_capacity() {
        let (writer, receiver) = endpoints(GATE_REGION_SIZE + 64);

        while write_frame(&writer, b"filler").is_ok() {}
        assert!(matches!(write_frame(&writer, b"filler"), Err(ClaimError::Capacity)));

        // Whatever fitted before the arena filled up is still readable.
        let reader = receiver.close().unwrap();
        assert!(reader.iter_frames().all(|frame| frame == b"filler"));
    }

    #[test]
    fn write_encoded_roundtrips_through_the_gate() {
        let (writer, receiver) = endpoints(1024);

        writer.write_encoded(&4242u32).unwrap();

        let reader = receiver.close().unwrap();
        let frame = reader.iter_frames().next().unwrap();
        assert_eq!(wincode::deserialize_exact::<u32>(frame).unwrap(), 4242);
    }

    #[test]
    fn concurrent_writers_are_all_published_by_a_successful_close() {
        const WRITERS: usize = 4;
        const PER_WRITER: usize = 20;

        let region = MockRegion::alloc(64 * 1024);
        // SAFETY: as in `endpoints`.
        let receiver = unsafe { GatedShmReceiver::new(region.clone()) };

        thread::scope(|scope| {
            for writer_index in 0..WRITERS {
                let region = region.clone();
                scope.spawn(move || {
                    // SAFETY: as in `endpoints`.
                    let writer = unsafe { GatedShmWriter::new(region) };
                    for frame_index in 0..PER_WRITER {
                        write_frame(&writer, format!("{writer_index} {frame_index}").as_bytes())
                            .unwrap();
                    }
                });
            }
        });

        // Every writer joined before the close, so it must report a clean gate,
        // and that `Ok` is also the synchronization that makes their frames
        // visible here.
        let reader = receiver.close().unwrap();
        let mut seen: Vec<&str> =
            reader.iter_frames().map(|frame| from_utf8(frame).unwrap()).collect();
        seen.sort_unstable();

        assert_eq!(seen.len(), WRITERS * PER_WRITER);
        assert_eq!(seen[0], "0 0");
    }
}

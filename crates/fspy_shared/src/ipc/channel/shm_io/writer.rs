//! The writer side: claiming, filling, and committing frames.

use std::{
    mem::ManuallyDrop,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
};

use wincode::{SchemaWrite, Serialize as _, config::DefaultConfig};

use super::{
    AsRawSlice, slot,
    state::{ReserveError, SharedState},
};

/// A concurrent shared-memory frame writer.
///
/// Safe to use across threads and processes at the same time: frames are
/// reserved with atomic operations, filled in uniquely owned payload spans,
/// and published with an atomic commit (see the module docs of
/// [`super::state`]).
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
    /// The frame is oversized or the region is full. The claim has already
    /// recorded the loss, so the channel will report itself incomplete.
    #[error("no space left in the shared-memory region")]
    Capacity,
}

#[derive(thiserror::Error, Debug)]
pub enum WriteEncodedError {
    #[error("failed to encode value into shared memory")]
    EncodeError(#[from] wincode::error::WriteError),
    #[error("tried to write a frame of zero size into shared memory")]
    ZeroSizedFrame,
    #[error("encoded size diverged from the declared serialized size")]
    SizeMismatch,
    #[error(transparent)]
    Claim(#[from] ClaimError),
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
    /// Panics when the region is not word-aligned or its size is outside the
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
    /// commits it. Dropping the frame without finishing abandons the claim
    /// and marks the channel incomplete.
    pub fn claim_frame(&self, frame_size: NonZeroUsize) -> Result<FrameMut<'_>, ClaimError> {
        let state = self.state();
        let reservation = state.try_claim(frame_size.get()).map_err(|err| match err {
            ReserveError::Closed => ClaimError::Closed,
            ReserveError::Capacity => {
                // The record is lost but this process lives on to perform the
                // operation, so flag the channel incomplete before the caller
                // proceeds.
                state.flag_incomplete();
                ClaimError::Capacity
            }
        })?;

        let content_ptr = state.payload_ptr(reservation.payload_offset);
        // SAFETY: the allocator compare-and-swap reserved
        // `[payload_offset, payload_offset + frame_size)` exclusively for
        // this frame: other writers reserve disjoint spans, and the receiver
        // never reads a payload before observing its committed descriptor —
        // which `finish` publishes only when it consumes this borrow.
        let content = unsafe { std::slice::from_raw_parts_mut(content_ptr, frame_size.get()) };
        Ok(FrameMut {
            state,
            slot_index: reservation.slot_index,
            descriptor: slot::committed(reservation.payload_offset, frame_size.get()),
            content,
        })
    }

    /// Writes one encoded value as a committed frame.
    pub fn write_encoded<T: SchemaWrite<DefaultConfig, Src = T>>(
        &self,
        value: &T,
    ) -> Result<(), WriteEncodedError> {
        let result = self.write_encoded_inner(value);
        if let Err(err) = &result
            && !matches!(err, WriteEncodedError::Claim(_))
        {
            // A pre-claim failure also loses a record this live process will
            // still act on; claim errors have already been recorded (or are
            // an ordinary post-close skip).
            self.state().flag_incomplete();
        }
        result
    }

    fn write_encoded_inner<T: SchemaWrite<DefaultConfig, Src = T>>(
        &self,
        value: &T,
    ) -> Result<(), WriteEncodedError> {
        let serialized_size =
            usize::try_from(T::serialized_size(value)?).expect("serialized size exceeds usize");

        let Some(frame_size) = NonZeroUsize::new(serialized_size) else {
            return Err(WriteEncodedError::ZeroSizedFrame);
        };
        let mut frame = self.claim_frame(frame_size)?;

        let mut writer: &mut [u8] = &mut frame;
        T::serialize_into(&mut writer, value)?;
        if !writer.is_empty() {
            // Dropping the partially filled frame leaves it unpublished.
            return Err(WriteEncodedError::SizeMismatch);
        }

        frame.finish();
        Ok(())
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
/// payload visible to the receiver. Dropping the frame instead abandons the
/// claim: the slot stays unfinished (the receiver will abort and ignore it)
/// and the channel is marked incomplete, because the dropping process is alive
/// to perform the operation this record was meant to describe. A process
/// that dies mid-frame runs no drop code and marks nothing — correctly so,
/// since records are published before the recorded operation is performed.
pub struct FrameMut<'a> {
    state: SharedState<'a>,
    slot_index: usize,
    descriptor: u64,
    content: &'a mut [u8],
}

impl std::fmt::Debug for FrameMut<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    /// first, the frame is silently discarded: the access belongs to the
    /// closed run's boundary race and is intentionally excluded either way.
    pub fn finish(self) {
        let this = ManuallyDrop::new(self);
        this.state.commit(this.slot_index, this.descriptor);
    }
}

impl Drop for FrameMut<'_> {
    fn drop(&mut self) {
        self.state.flag_incomplete();
    }
}

use std::io;

use fspy_shared::ipc::{
    PathAccess,
    channel::{Frames, Receiver},
};
use tokio::task::spawn_blocking;

// Shared memory size for storing path accesses.
// 4 GiB is large enough to store path accesses in almost any realistic scenario.
// This doesn't allocate physical memory until it's actually used.
pub const SHM_CAPACITY: usize = 4 * 1024 * 1024 * 1024;

/// The validated path accesses collected from a closed IPC channel.
pub struct CollectedAccesses {
    frames: Frames,
}

impl CollectedAccesses {
    /// Closes the channel and validates the collected trace.
    ///
    /// Never waits for tracked processes: closing rejects new records and
    /// atomically ignores unfinished ones (see
    /// [`fspy_shared::ipc::channel::Receiver::close`]).
    ///
    /// Fails when the trace cannot back the run's file accesses: a record
    /// was lost before close, or the shared memory was corrupted. Failing
    /// here — instead of returning a silently short trace — keeps the
    /// tracking result trustworthy for caching.
    pub fn collect(receiver: Receiver) -> io::Result<Self> {
        let frames = receiver.close()?;
        if !frames.is_complete() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file-access trace is incomplete: a tracked process lost a record",
            ));
        }
        // Validate every frame once so iteration is infallible.
        for frame in frames.iter() {
            let _: PathAccess<'_> = wincode::deserialize_exact(frame).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("corrupt file-access record: {err}"),
                )
            })?;
        }
        Ok(Self { frames })
    }

    pub async fn collect_async(receiver: Receiver) -> io::Result<Self> {
        spawn_blocking(move || Self::collect(receiver)).await.expect("collect task panicked")
    }

    pub fn iter_path_accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.frames
            .iter()
            .map(|frame| wincode::deserialize_exact(frame).expect("frames validated in collect"))
    }
}

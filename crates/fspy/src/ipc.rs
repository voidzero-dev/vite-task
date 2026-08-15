use std::io;

use fspy_shared::ipc::{
    PathAccess,
    channel::{Frames, Receiver},
};

// Payload budget for path-access records; the channel adds its fixed
// descriptor table on top. 4 GiB is large enough for almost any realistic
// scenario, and none of it occupies physical memory until actually used.
pub const SHM_CAPACITY: usize = 4 * 1024 * 1024 * 1024;

/// The path accesses a run reported through the IPC channel.
pub struct ChannelAccesses {
    frames: Frames,
}

impl TryFrom<Receiver> for ChannelAccesses {
    type Error = io::Error;

    /// Closes the channel and rejects traces that cannot back the run's
    /// file accesses.
    ///
    /// Never waits for tracked processes — closing rejects new records and
    /// atomically ignores unfinished ones (see
    /// [`fspy_shared::ipc::channel::Receiver::close`]) — and its work is
    /// bounded by the number of reported records, so it runs inline.
    ///
    /// Fails when a record was lost before close or the shared-memory
    /// metadata was corrupted. Failing here — instead of returning a
    /// silently short trace — keeps the tracking result trustworthy for
    /// caching.
    fn try_from(receiver: Receiver) -> io::Result<Self> {
        let frames = receiver.close()?;
        if !frames.is_complete() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file-access trace is incomplete: a tracked process lost a record",
            ));
        }
        Ok(Self { frames })
    }
}

impl ChannelAccesses {
    pub fn iter_path_accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.frames.iter().map(|frame| {
            wincode::deserialize_exact(frame)
                .expect("committed frames are complete under the channel protocol")
        })
    }
}

use std::io;

use fspy_shared::ipc::{
    PathAccess,
    channel::{FrameReader, Receiver},
};

// Shared memory region size: the channel's fixed descriptor table plus
// ~3.5 GiB of payload room — enough path accesses for almost any realistic
// scenario. None of it occupies physical memory until actually used.
pub const SHM_CAPACITY: usize = 4 * 1024 * 1024 * 1024;

/// The path accesses a run reported through the IPC channel.
pub struct ChannelAccesses {
    frames: FrameReader,
}

impl TryFrom<Receiver> for ChannelAccesses {
    type Error = io::Error;

    /// Closes the channel and rejects traces that cannot back the run's
    /// file accesses.
    ///
    /// Never waits for tracked processes: closing reads one counter and
    /// shuts the channel's gate (see
    /// [`fspy_shared::ipc::channel::Receiver::close`]), so it runs inline
    /// however many records were reported.
    ///
    /// Fails when a record was lost before close, which is what keeps the
    /// tracking result trustworthy for caching: closing hands back frames
    /// only when they are all of them. A run that fills the region
    /// therefore fails here rather than reporting a short trace.
    fn try_from(receiver: Receiver) -> io::Result<Self> {
        Ok(Self { frames: receiver.close()? })
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

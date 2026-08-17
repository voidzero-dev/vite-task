use fspy_shared::ipc::{
    PathAccess,
    channel::{FrameReader, Receiver},
};

/// The path accesses a run reported through the IPC channel.
pub struct ChannelAccesses {
    /// `None` when a record was lost, which leaves what did arrive too
    /// incomplete to build anything on.
    frames: Option<FrameReader>,
}

impl From<Receiver> for ChannelAccesses {
    /// Closes the channel and keeps its frames, unless a sender ran out of
    /// room.
    ///
    /// Never waits for tracked processes: closing reads one counter and
    /// shuts the channel's gate (see
    /// [`fspy_shared::ipc::channel::Receiver::close`]), so it runs inline
    /// however many records were reported.
    fn from(receiver: Receiver) -> Self {
        Self { frames: receiver.close().ok() }
    }
}

impl ChannelAccesses {
    /// Whether every record senders published is here.
    ///
    /// `false` means a sender could not record something it then went on
    /// to do, so what follows is a subset of what the run really touched.
    /// Anything that needs all of them — caching, above all — has to treat
    /// the run as untracked rather than as having touched only these
    /// paths.
    pub const fn is_complete(&self) -> bool {
        self.frames.is_some()
    }

    pub fn iter_path_accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.frames.iter().flat_map(|frames| {
            frames.iter().map(|frame| {
                wincode::deserialize_exact(frame)
                    .expect("committed frames are complete under the channel protocol")
            })
        })
    }
}

use fspy_shared::ipc::{
    ChannelSize, PathAccess,
    channel::{FrameReader, Receiver},
};

use crate::error::TrackingIncomplete;

/// Shared memory for one tracked run's file-access records.
///
/// 4 GiB of sparse address space: none of it becomes real memory until
/// records land in it, and it leaves room for tens of millions of
/// accesses.
const DEFAULT_SHM_CAPACITY: usize = 4 * 1024 * 1024 * 1024;

/// Overrides [`DEFAULT_SHM_CAPACITY`] with a byte count. Internal: it
/// exists so a test can shrink the region until a run overruns it, and
/// nothing outside this repository should set it.
const SHM_CAPACITY_ENV: &str = "VP_RUN_INTERNAL_FSPY_SHM_CAPACITY";

/// How much shared memory to give the next tracked run, split at one
/// record per 64 bytes: 8 for its descriptor and 56 for its payload.
/// Records run a few hundred bytes each, so payload space runs out well
/// before slots do.
///
/// # Panics
///
/// When the override is set to something that is not a byte count. It is
/// ours to set, so a value we cannot read is a mistake worth stopping for
/// rather than quietly ignoring.
pub fn shm_size() -> ChannelSize {
    let capacity = std::env::var_os(SHM_CAPACITY_ENV).map_or(DEFAULT_SHM_CAPACITY, |value| {
        value.to_str().and_then(|value| value.parse().ok()).unwrap_or_else(|| {
            panic!("{SHM_CAPACITY_ENV} is not a byte count: {}", value.display())
        })
    });
    ChannelSize { capacity, slots: capacity / 64 }
}

/// The path accesses a run reported through the IPC channel.
pub struct ChannelAccesses {
    frames: FrameReader,
}

impl TryFrom<Receiver> for ChannelAccesses {
    type Error = TrackingIncomplete;

    /// Closes the channel and takes every record it collected.
    ///
    /// Never waits for tracked processes: closing reads one counter and
    /// shuts the channel's gate (see
    /// [`fspy_shared::ipc::channel::Receiver::close`]), so it runs inline
    /// however many records were reported.
    ///
    /// # Errors
    ///
    /// [`TrackingIncomplete`] when a tracked process could not record
    /// something it went on to do. What did arrive is then a subset of
    /// what the run really touched, so none of it is handed back.
    fn try_from(receiver: Receiver) -> Result<Self, TrackingIncomplete> {
        Ok(Self { frames: receiver.close().map_err(|_| TrackingIncomplete)? })
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

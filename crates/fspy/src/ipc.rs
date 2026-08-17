use std::io;

use fspy_shared::ipc::{
    PathAccess,
    channel::{Receiver, ReceiverLockGuard},
};
use tokio::task::spawn_blocking;

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

/// How much shared memory to give the next tracked run.
///
/// # Panics
///
/// When the override is set to something that is not a byte count. It is
/// ours to set, so a value we cannot read is a mistake worth stopping for
/// rather than quietly ignoring.
pub fn shm_capacity() -> usize {
    std::env::var_os(SHM_CAPACITY_ENV).map_or(DEFAULT_SHM_CAPACITY, |value| {
        value.to_str().and_then(|value| value.parse().ok()).unwrap_or_else(|| {
            panic!("{SHM_CAPACITY_ENV} is not a byte count: {}", value.display())
        })
    })
}

#[ouroboros::self_referencing]
pub struct OwnedReceiverLockGuard {
    /// Owns the shared memory
    receiver: Receiver,
    /// Borrows the shared memory and owns the file lock
    #[borrows(receiver)]
    #[covariant]
    lock_guard: ReceiverLockGuard<'this>,
}

impl OwnedReceiverLockGuard {
    pub fn lock(receiver: Receiver) -> io::Result<Self> {
        Self::try_new(receiver, fspy_shared::ipc::channel::Receiver::lock)
    }

    pub async fn lock_async(receiver: Receiver) -> io::Result<Self> {
        spawn_blocking(move || Self::lock(receiver)).await.expect("lock task panicked")
    }

    pub fn iter_path_accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
        self.borrow_lock_guard()
            .iter_frames()
            .map(|frame| wincode::deserialize_exact(frame).unwrap())
    }
}

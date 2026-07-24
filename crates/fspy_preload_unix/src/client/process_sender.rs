use std::{
    io,
    sync::atomic::{AtomicPtr, Ordering},
};

use fspy_shared::ipc::channel::{ChannelConf, Sender};

use super::fork_tracker::{ForkGeneration, ForkTracker};

struct SenderState {
    generation: ForkGeneration,
    sender: Option<Sender>,
}

pub(super) struct ProcessSender {
    conf: ChannelConf,
    fork_tracker: ForkTracker,
    state: AtomicPtr<SenderState>,
}

impl ProcessSender {
    #[cfg_attr(
        test,
        expect(dead_code, reason = "the preload constructor is disabled in unit-test builds")
    )]
    pub(super) fn new(conf: &ChannelConf) -> nix::Result<Self> {
        let fork_tracker = ForkTracker::new()?;
        let generation = fork_tracker.generation();
        let state = Box::into_raw(Box::new(SenderState::new(generation, conf)));

        Ok(Self { conf: conf.clone(), fork_tracker, state: AtomicPtr::new(state) })
    }

    pub(super) fn sender(&self) -> Option<&Sender> {
        loop {
            let generation = self.fork_tracker.generation();
            let current_ptr = self.state.load(Ordering::Acquire);
            // SAFETY: Published states are never reclaimed, so the pointer
            // remains valid for the process lifetime.
            let current = unsafe { &*current_ptr };
            if current.generation == generation {
                return current.sender.as_ref();
            }

            let replacement_ptr = Box::into_raw(Box::new(SenderState::new(generation, &self.conf)));
            match self.state.compare_exchange(
                current_ptr,
                replacement_ptr,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // Do not reclaim `current_ptr`. After fork it can own an
                    // inherited sender whose destructor must not affect the
                    // parent's file description. In the original process,
                    // another thread may still be borrowing the old state.
                    // SAFETY: This state was just allocated and published, and
                    // published states are never reclaimed.
                    return unsafe { &*replacement_ptr }.sender.as_ref();
                }
                Err(_) => {
                    // SAFETY: This allocation was never published, so no other
                    // thread can reference it.
                    drop(unsafe { Box::from_raw(replacement_ptr) });
                }
            }
        }
    }
}

impl SenderState {
    #[expect(
        clippy::print_stderr,
        reason = "preload library intentionally reports sender initialization failures"
    )]
    fn new(generation: ForkGeneration, conf: &ChannelConf) -> Self {
        let sender = match retry_interrupted(|| conf.sender()) {
            Ok(sender) => Some(sender),
            Err(error) => {
                // The receiver may already be closed when a late child first
                // accesses the filesystem. Keep the traced process running and
                // cache the failure for this process generation.
                eprintln!("fspy: failed to create ipc sender: {error}");
                None
            }
        };
        Self { generation, sender }
    }
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_sender_creation_is_retried() {
        let mut attempts = 0;
        let value = retry_interrupted(|| {
            attempts += 1;
            if attempts < 3 { Err(io::ErrorKind::Interrupted.into()) } else { Ok(42) }
        })
        .unwrap();

        assert_eq!(value, 42);
        assert_eq!(attempts, 3);
    }
}

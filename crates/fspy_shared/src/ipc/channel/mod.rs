//! Fast mpsc IPC channel implementation based on shared memory.

use std::{env::temp_dir, fs::File, io, ops::Deref, path::PathBuf, sync::Arc};

use bincode::{Decode, Encode};
#[cfg(not(target_os = "android"))]
use shared_memory::ShmemConf;
use shm_io::{ShmReader, ShmWriter};
use tracing::debug;
use uuid::Uuid;

use super::NativeStr;

mod backend;
mod shm_io;

#[derive(Encode, Decode, Clone, Debug)]
pub struct ChannelConf {
    lock_file_path: Box<NativeStr>,
    shm_id: Arc<str>,
    shm_size: usize,
}

pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    let lock_file_path = temp_dir().join(format!("fspy_ipc_{}.lock", Uuid::new_v4()));

    let shm = backend::create(capacity)?;

    #[cfg(target_os = "android")]
    let conf = ChannelConf {
        lock_file_path: lock_file_path.as_os_str().into(),

        shm_id: shm.fd_path.clone(),
        shm_size: capacity,
    };

    #[cfg(not(target_os = "android"))]
    let conf = ChannelConf {
        lock_file_path: lock_file_path.as_os_str().into(),

        shm_id: shm.os_id.clone().into(),
        shm_size: capacity,
    };

    let receiver = Receiver::new(lock_file_path, shm)?;
    Ok((conf, receiver))
}

impl ChannelConf {
    /// Creates a sender.
    ///
    /// This doesn't block on the file lock. Instead it returns immediately with error if the receiver is locked or dropped.
    #[expect(
        clippy::missing_errors_doc,
        reason = "error conditions are self-evident from return type"
    )]
    pub fn sender(&self) -> io::Result<Sender> {
        let lock_file = File::open(self.lock_file_path.to_cow_os_str())?;
        lock_file.try_lock_shared()?;

        #[cfg(target_os = "android")]
        let writer = {
            let mmap = backend::open(&self.shm_id, self.shm_size)?;

            WriterHandle::MemFd(unsafe { ShmWriter::new(mmap) })
        };

        #[cfg(not(target_os = "android"))]
        let writer = {
            #[cfg_attr(
                not(windows),
                expect(unused_mut, reason = "mut required on Windows, unused on Unix")
            )]
            let mut conf = ShmemConf::new().size(self.shm_size).os_id(&self.shm_id);

            #[cfg(target_os = "windows")]
            {
                conf = conf.allow_raw(true);
            }

            let shm = conf.open().map_err(io::Error::other)?;

            // SAFETY: shm valide + lock garantit exclusivité
            unsafe { ShmWriter::new(shm) }
        };

        Ok(Sender { writer, lock_file, lock_file_path: self.lock_file_path.clone() })
    }
}

pub struct Sender {
    writer: WriterHandle,
    lock_file_path: Box<NativeStr>,
    lock_file: File,
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let Err(err) = self.lock_file.unlock() {
            debug!("Failed to unlock the shared IPC lock {:?}: {}", self.lock_file_path, err);
        }
    }
}

impl Deref for Sender {
    type Target = WriterHandle;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "`Sender` holds a shared file lock that ensures there's no reader, so `shm` can be safely written to"
)]
/// SAFETY: `Sender` holds a shared file lock that ensures there's no reader, so `shm` can be safely written to.
unsafe impl Send for Sender {}

/// SAFETY: `Sender` holds a shared file lock that ensures there's no reader, so `shm` can be safely written to.
unsafe impl Sync for Sender {}

enum ShmHandle {
    #[cfg(target_os = "android")]
    MemFd(memmap2::MmapMut),
    #[cfg(not(target_os = "android"))]
    Shmem(shared_memory::Shmem),
}

pub enum WriterHandle {
    #[cfg(target_os = "android")]
    MemFd(ShmWriter<memmap2::MmapMut>),
    #[cfg(not(target_os = "android"))]
    Shmem(ShmWriter<shared_memory::Shmem>),
}

pub struct Receiver {
    lock_file_path: PathBuf,
    lock_file: File,
    shm: ShmHandle,
}

unsafe impl Send for Receiver {}
unsafe impl Sync for Receiver {}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_file_path) {
            debug!("Failed to remove IPC lock file {:?}: {}", self.lock_file_path, err);
        }
    }
}

impl Receiver {
    fn new(lock_file_path: PathBuf, shm: backend::Shm) -> io::Result<Self> {
        let lock_file = File::create(&lock_file_path)?;

        #[cfg(target_os = "android")]
        let shm = ShmHandle::MemFd(shm.mmap);

        #[cfg(not(target_os = "android"))]
        let shm = ShmHandle::Shmem(shm.shm);

        Ok(Self { lock_file_path, lock_file, shm })
    }

    /// Lock the shared memory for unique read access.
    /// Blocks until all the senders have dropped (or processes owning them have all exited) so the shared memory can be safely read.
    /// During the lifetime of returned `ReceiverReadGuard`, no new senders can be created (`ChannelConf::sender` would fail).
    #[expect(
        clippy::missing_errors_doc,
        reason = "error conditions are self-evident from return type"
    )]
    pub fn lock(&self) -> io::Result<ReceiverLockGuard<'_>> {
        self.lock_file.lock()?;

        let slice: &[u8] = match &self.shm {
            #[cfg(target_os = "android")]
            ShmHandle::MemFd(mmap) => unsafe {
                std::slice::from_raw_parts(mmap.as_ptr(), mmap.len())
            },

            #[cfg(not(target_os = "android"))]
            ShmHandle::Shmem(shm) => unsafe { shm.as_slice() },
        };

        let reader = ShmReader::new(slice);

        Ok(ReceiverLockGuard { reader, lock_file: &self.lock_file })
    }
}

pub struct ReceiverLockGuard<'a> {
    reader: ShmReader<&'a [u8]>,
    lock_file: &'a File,
}

impl Drop for ReceiverLockGuard<'_> {
    fn drop(&mut self) {
        if let Err(err) = self.lock_file.unlock() {
            debug!("Failed to unlock IPC lock file: {}", err);
        }
    }
}
impl<'a> Deref for ReceiverLockGuard<'a> {
    type Target = ShmReader<&'a [u8]>;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, str::from_utf8};

    use bstr::B;
    use subprocess_test::command_for_fn;

    use super::*;

    #[test]
    fn smoke() {
        let (conf, receiver) = channel(100).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
        });
        assert!(std::process::Command::from(cmd).status().unwrap().success());

        let lock = receiver.lock().unwrap();
        let mut frames = lock.iter_frames();

        let received_frame = frames.next().unwrap();
        assert_eq!(received_frame, &[4, 2]);

        assert!(frames.next().is_none());
    }

    #[test]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    fn forbid_new_senders_after_locked() {
        let (conf, receiver) = channel(42).unwrap();
        let _lock = receiver.lock().unwrap();

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert_eq!(B(&output.stdout), B("false"));
    }

    #[test]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    fn forbid_new_senders_after_receiver_dropped() {
        let (conf, receiver) = channel(42).unwrap();
        drop(receiver);

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert_eq!(B(&output.stdout), B("false"));
    }

    #[test]
    fn concurrent_senders() {
        let (conf, receiver) = channel(8192).unwrap();
        for i in 0u16..200 {
            let cmd = command_for_fn!((conf.clone(), i), |(conf, i): (ChannelConf, u16)| {
                let sender = conf.sender().unwrap();
                let data_to_send = i.to_string();
                sender
                    .claim_frame(NonZeroUsize::new(data_to_send.len()).unwrap())
                    .unwrap()
                    .copy_from_slice(data_to_send.as_bytes());
            });
            let output = std::process::Command::from(cmd).output().unwrap();
            assert!(
                output.status.success(),
                "Failed to send in iteration {}: {:?}",
                i,
                B(&output.stderr)
            );
        }
        let lock = receiver.lock().unwrap();
        let mut received_values: Vec<u16> = lock
            .iter_frames()
            .map(|frame| from_utf8(frame).unwrap().parse::<u16>().unwrap())
            .collect();
        received_values.sort_unstable();
        assert_eq!(received_values, (0u16..200).collect::<Vec<u16>>());
    }
}

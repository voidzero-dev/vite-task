//! Fast mpsc IPC channel implementation based on shared memory.

mod shm_io;

use std::{
    borrow::Cow,
    env::temp_dir,
    ffi::OsStr,
    fs::File,
    io,
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};

use shared_memory::{Shmem, ShmemConf};
pub use shm_io::FrameMut;
use shm_io::{ShmReader, ShmWriter};
use tracing::debug;
use uuid::Uuid;
use wincode::{
    SchemaRead, SchemaWrite,
    config::{Config, DefaultConfig},
    error::{ReadResult, WriteResult},
    io::{Reader, Writer},
};

use super::NativeStr;

/// wincode schema adapter for `Arc<str>`, which is a foreign type with unsized inner.
pub(crate) struct ArcStrSchema;

// SAFETY: Delegates to `str`'s SchemaWrite impl, preserving its size/write invariants.
unsafe impl<C: Config> SchemaWrite<C> for ArcStrSchema {
    type Src = Arc<str>;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        <str as SchemaWrite<C>>::size_of(src)
    }

    fn write(writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        <str as SchemaWrite<C>>::write(writer, src)
    }
}

// SAFETY: Delegates to `&str`'s SchemaRead impl; dst is initialized on Ok.
unsafe impl<'de, C: Config> SchemaRead<'de, C> for ArcStrSchema {
    type Dst = Arc<str>;

    fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self::Dst>) -> ReadResult<()> {
        let s: &str = <&str as SchemaRead<'de, C>>::get(&mut reader)?;
        dst.write(Arc::from(s));
        Ok(())
    }
}

/// Serializable configuration to create channel senders.
#[derive(SchemaWrite, SchemaRead, Clone, Debug)]
pub struct ChannelConf {
    lock_file_path: Box<NativeStr>,
    #[wincode(with = "ArcStrSchema")]
    shm_id: Arc<str>,
    shm_size: usize,
}

/// Creates a mpsc IPC channel with one receiver and a `ChannelConf` that can be passed around processes and used to create multiple senders
#[expect(
    clippy::missing_errors_doc,
    reason = "non-vite crate: cannot use vite_str/vite_path types"
)]
pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    // Initialize the lock file with a unique name.
    let lock_file_path = temp_dir().join(format!("fspy_ipc_{}.lock", Uuid::new_v4()));

    #[cfg_attr(
        not(windows),
        expect(unused_mut, reason = "mut required on Windows, unused on Unix")
    )]
    let mut conf = ShmemConf::new().size(capacity);
    // On Windows, allow opening raw shared memory (without backing file) for DLL injection scenarios
    #[cfg(target_os = "windows")]
    {
        conf = conf.allow_raw(true);
    }

    let shm = conf.create().map_err(io::Error::other)?;

    let conf = ChannelConf {
        lock_file_path: lock_file_path.as_os_str().into(),
        shm_id: shm.get_os_id().into(),
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

        #[cfg_attr(
            not(windows),
            expect(unused_mut, reason = "mut required on Windows, unused on Unix")
        )]
        let mut conf = ShmemConf::new().size(self.shm_size).os_id(&self.shm_id);
        // On Windows, allow opening raw shared memory (without backing file) for DLL injection scenarios
        #[cfg(target_os = "windows")]
        {
            conf = conf.allow_raw(true);
        }
        let shm = match conf.open().map_err(io::Error::other) {
            Ok(shm) => shm,
            Err(error) => {
                let _ = lock_file.unlock();
                return Err(error);
            }
        };
        lock_file.unlock()?;
        // SAFETY: `shm` is a freshly opened shared memory region with valid pointer and size.
        // Individual writes acquire the shared file lock before accessing it.
        let writer = unsafe { ShmWriter::new(shm) };
        Ok(Sender { writer, lock_file_path: self.lock_file_path.clone() })
    }
}

pub struct Sender {
    writer: ShmWriter<Shmem>,
    lock_file_path: Box<NativeStr>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimFrameError {
    #[error("The channel receiver has started collecting or was dropped")]
    Closed(#[source] io::Error),
    #[error("Failed to acquire the channel write lock")]
    Lock(#[source] io::Error),
    #[error("Not enough space remains in the shared-memory channel")]
    InsufficientSpace,
}

#[derive(Debug, thiserror::Error)]
pub enum SenderWriteError {
    #[error(transparent)]
    Channel(#[from] ClaimFrameError),
    #[error(transparent)]
    Encode(#[from] shm_io::WriteEncodedError),
}

impl Sender {
    #[must_use]
    pub fn lock_file_path(&self) -> Cow<'_, OsStr> {
        self.lock_file_path.to_cow_os_str()
    }

    #[must_use]
    pub fn is_lock_file(&self, path: &std::path::Path) -> bool {
        path.as_os_str() == self.lock_file_path.to_cow_os_str()
    }

    #[must_use]
    pub fn is_lock_native_path(&self, path: &super::NativePath) -> bool {
        path.strip_path_prefix(self.lock_file_path.to_cow_os_str(), |stripped| {
            stripped.is_ok_and(|stripped| stripped.as_os_str().is_empty())
        })
    }

    /// Claims a frame while holding the receiver exclusion lock.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver has started collecting, the lock cannot
    /// be acquired, or the channel has insufficient space.
    pub fn claim_frame(
        &self,
        frame_size: std::num::NonZeroUsize,
    ) -> Result<SenderFrame<'_>, ClaimFrameError> {
        let lock_file = self.open_lock_file()?;
        // SAFETY: `open_lock_file` opens this sender's exact lock path and
        // transfers the only handle ownership into the returned frame.
        unsafe { self.claim_frame_with_lock(frame_size, lock_file) }
    }

    /// Claims a frame using an independently opened lock-file handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver has started collecting, the lock cannot
    /// be acquired, or the channel has insufficient space.
    ///
    /// # Safety
    ///
    /// `lock_file` must be a newly opened handle for this sender's exact lock
    /// file. No alias may unlock it before the returned frame is dropped.
    pub unsafe fn claim_frame_with_lock(
        &self,
        frame_size: std::num::NonZeroUsize,
        lock_file: File,
    ) -> Result<SenderFrame<'_>, ClaimFrameError> {
        Self::lock_for_write(&lock_file)?;
        let Some(frame) = self.writer.claim_frame(frame_size) else {
            let _ = lock_file.unlock();
            return Err(ClaimFrameError::InsufficientSpace);
        };
        Ok(SenderFrame {
            frame: ManuallyDrop::new(frame),
            lock_file,
            lock_file_path: &self.lock_file_path,
        })
    }

    /// Encodes a value while holding the receiver exclusion lock.
    ///
    /// # Errors
    ///
    /// Returns an error if the receiver has started reading, encoding fails, or
    /// the shared-memory channel has insufficient space.
    pub fn write_encoded<T: SchemaWrite<DefaultConfig, Src = T>>(
        &self,
        value: &T,
    ) -> Result<(), SenderWriteError> {
        let lock_file = self.open_lock_file()?;
        Self::lock_for_write(&lock_file)?;
        let result = self.writer.write_encoded(value).map_err(SenderWriteError::from);
        let unlock_result = lock_file
            .unlock()
            .map_err(|error| SenderWriteError::Channel(ClaimFrameError::Lock(error)));
        result?;
        unlock_result
    }

    fn open_lock_file(&self) -> Result<File, ClaimFrameError> {
        File::open(self.lock_file_path.to_cow_os_str()).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound
                || (cfg!(unix) && error.raw_os_error() == Some(38))
            {
                ClaimFrameError::Closed(error)
            } else {
                ClaimFrameError::Lock(error)
            }
        })
    }

    fn lock_for_write(lock_file: &File) -> Result<(), ClaimFrameError> {
        lock_file.try_lock_shared().map_err(|error| {
            let error: io::Error = error.into();
            if error.kind() == io::ErrorKind::WouldBlock {
                ClaimFrameError::Closed(error)
            } else {
                ClaimFrameError::Lock(error)
            }
        })
    }
}

pub struct SenderFrame<'a> {
    frame: ManuallyDrop<FrameMut<'a>>,
    lock_file: File,
    lock_file_path: &'a NativeStr,
}

impl Deref for SenderFrame<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.frame[..]
    }
}

impl DerefMut for SenderFrame<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame[..]
    }
}

impl Drop for SenderFrame<'_> {
    fn drop(&mut self) {
        // Complete the frame before releasing the receiver exclusion lock.
        // SAFETY: `frame` is dropped exactly once here.
        unsafe { ManuallyDrop::drop(&mut self.frame) };
        if let Err(err) = self.lock_file.unlock() {
            debug!("Failed to unlock the shared IPC lock {:?}: {}", self.lock_file_path, err);
        }
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "`Sender` acquires a shared file lock around each write, so `shm` can be safely written to"
)]
/// SAFETY: `Sender` acquires a shared file lock around each write, so `shm` can be safely written to.
unsafe impl Send for Sender {}

/// SAFETY: `Sender` acquires a shared file lock around each write, so `shm` can be safely written to.
unsafe impl Sync for Sender {}

/// The unique receiver side of an IPC channel.
/// Owns the lock file and removes it on drop.
pub struct Receiver {
    lock_file_path: PathBuf,
    lock_file: File,
    shm: Shmem,
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "Receiver doesn't read or write `shm`. It only pass it to `ReceiverLockGuard` under the lock"
)]
/// SAFETY: `Receiver` doesn't read or write `shm`. It only passes it to `ReceiverLockGuard` under the lock.
unsafe impl Send for Receiver {}

/// SAFETY: `Receiver` doesn't read or write `shm`. It only passes it to `ReceiverLockGuard` under the lock.
unsafe impl Sync for Receiver {}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_file_path) {
            debug!("Failed to remove IPC lock file {:?}: {}", self.lock_file_path, err);
        }
    }
}

impl Receiver {
    fn new(lock_file_path: PathBuf, shm: Shmem) -> io::Result<Self> {
        let lock_file = File::create(&lock_file_path)?;
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
        // SAFETY: The exclusive file lock is held, so no writers can access the shared memory.
        // The lock ensures all prior writes are visible to this thread.
        let reader = ShmReader::new(unsafe { self.shm.as_slice() });
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
    use std::{
        num::NonZeroUsize,
        str::from_utf8,
        sync::mpsc::{self, RecvTimeoutError},
        thread,
        time::Duration,
    };

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
    fn existing_sender_does_not_block_receiver_lock() {
        let (conf, receiver) = channel(100).unwrap();
        let sender = conf.sender().unwrap();
        sender.claim_frame(NonZeroUsize::new(3).unwrap()).unwrap().copy_from_slice(b"old");

        let lock = receiver.lock().unwrap();
        assert_eq!(lock.iter_frames().collect::<Vec<_>>(), vec![b"old"]);
        assert!(matches!(
            sender.claim_frame(NonZeroUsize::new(3).unwrap()),
            Err(ClaimFrameError::Closed(_))
        ));
    }

    #[test]
    fn overlapping_frames_from_one_sender_hold_independent_locks() {
        let (conf, receiver) = channel(100).unwrap();
        let sender = conf.sender().unwrap();
        let mut first = sender.claim_frame(NonZeroUsize::new(3).unwrap()).unwrap();
        first.copy_from_slice(b"one");
        let mut second = sender.claim_frame(NonZeroUsize::new(3).unwrap()).unwrap();
        second.copy_from_slice(b"two");

        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let lock = receiver.lock().unwrap();
            let frames = lock.iter_frames().map(<[u8]>::to_vec).collect::<Vec<_>>();
            tx.send(frames).unwrap();
        });

        assert_eq!(rx.recv_timeout(Duration::from_millis(50)), Err(RecvTimeoutError::Timeout));
        drop(first);
        assert_eq!(rx.recv_timeout(Duration::from_millis(50)), Err(RecvTimeoutError::Timeout));
        drop(second);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            vec![b"one".to_vec(), b"two".to_vec()]
        );
        reader.join().unwrap();
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

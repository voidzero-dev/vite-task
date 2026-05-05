//! Fast mpsc IPC channel implementation based on shared memory.

mod shm_io;

use std::{env::temp_dir, fs::File, io, mem::MaybeUninit, ops::Deref, path::PathBuf, sync::Arc};

use shared_memory::{Shmem, ShmemConf};
pub use shm_io::FrameMut;
use shm_io::{ShmReader, ShmWriter};
use tracing::debug;
use uuid::Uuid;
use wincode::{
    SchemaRead, SchemaWrite,
    config::Config,
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
        let shm = conf.open().map_err(io::Error::other)?;
        // SAFETY: `shm` is a freshly opened shared memory region with valid pointer and size.
        // Exclusive write access is ensured by the shared file lock held by this sender.
        let writer = unsafe { ShmWriter::new(shm) };
        Ok(Sender { writer, lock_file, lock_file_path: self.lock_file_path.clone() })
    }
}

pub struct Sender {
    writer: ShmWriter<Shmem>,
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
    type Target = ShmWriter<Shmem>;

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

    /// Regression test for <https://github.com/voidzero-dev/vite-plus/issues/1453>.
    ///
    /// The current implementation backs the channel with POSIX shared memory
    /// (`shm_open`), which stores its file under `/dev/shm`. On hosts where
    /// `/dev/shm` is size-capped (e.g. Docker's 64 MiB default) a workload
    /// whose path-access stream exceeds that cap triggers `SIGBUS` in the
    /// sender when tmpfs can't allocate the next page. `cache: false` works
    /// around it by skipping fspy entirely.
    ///
    /// This test reproduces the crash without `sudo` and without needing the
    /// test environment itself to have a small `/dev/shm`: it enters an
    /// unprivileged user+mount namespace in a subprocess and remounts
    /// `/dev/shm` as a 1 MiB tmpfs, then writes past the cap via the real
    /// `channel()` API. The test asserts the subprocess completes cleanly;
    /// today it dies from `SIGBUS`. Switching the backing store to
    /// `memfd_create` (which is sized against RAM + overcommit, not
    /// `/dev/shm`) will let this test pass unchanged — the subprocess's
    /// `/dev/shm` constraint becomes irrelevant.
    #[test]
    #[cfg(target_os = "linux")]
    #[cfg_attr(miri, ignore = "miri can't mmap or unshare")]
    fn channel_survives_constrained_dev_shm() {
        use std::os::unix::process::ExitStatusExt;

        // Capacity chosen to comfortably exceed the 1 MiB tmpfs cap. The
        // `ftruncate` inside `shared_memory` is lazy on tmpfs, so this
        // allocation itself succeeds; the crash happens when the sender
        // later writes into pages that tmpfs can no longer back.
        const CAPACITY: usize = 16 * 1024 * 1024;

        let cmd = command_for_fn!((), |(): ()| {
            enter_userns_with_small_dev_shm();

            let (conf, _receiver) = super::channel(CAPACITY).expect("channel creation");
            let sender = conf.sender().expect("sender creation");

            // Claim a single 4 MiB frame and fill it byte-by-byte. The
            // first ~1 MiB of writes fit within the tmpfs quota; the next
            // byte faults on an un-backed page -> SIGBUS.
            let frame_size = NonZeroUsize::new(4 * 1024 * 1024).unwrap();
            let mut frame = sender.claim_frame(frame_size).expect("claim_frame");
            frame.fill(0xAB);
        });

        let status = std::process::Command::from(cmd).status().unwrap();

        assert!(
            status.success(),
            "channel writes should survive a constrained /dev/shm, but the \
             subprocess exited abnormally: code={:?} signal={:?}. \
             SIGBUS ({sigbus}) indicates the issue #1453 reproduction: tmpfs \
             page allocation failed on a write to the shm-backed mapping.",
            status.code(),
            status.signal(),
            sigbus = nix::sys::signal::Signal::SIGBUS as i32,
        );
    }

    /// Procfs files must be opened without `O_CREAT` — synthetic inodes
    /// reject the create bit on some hosts with `EACCES`. `std::fs::write`
    /// uses `File::create` (which sets `O_CREAT`), so we can't use it here.
    #[cfg(target_os = "linux")]
    fn write_procfs(path: &str, content: &str) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
        f.write_all(content.as_bytes())
    }

    /// Enter a fresh user + mount namespace in which the current uid is
    /// mapped to 0, then remount `/dev/shm` as a 1 MiB tmpfs. Must be called
    /// before any threads are spawned in the current process. Panics on any
    /// failure — unprivileged user namespace support is a hard requirement
    /// for this reproduction.
    #[cfg(target_os = "linux")]
    fn enter_userns_with_small_dev_shm() {
        use nix::mount::{MsFlags, mount};
        use nix::sched::{CloneFlags, unshare};
        use nix::unistd::{Gid, Uid};

        let uid = Uid::current().as_raw();
        let gid = Gid::current().as_raw();

        unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)
            .expect("unshare(CLONE_NEWUSER|CLONE_NEWNS)");

        // Inside the new user namespace the current process starts as
        // "nobody" until the id maps are written.
        write_procfs("/proc/self/uid_map", &std::format!("0 {uid} 1\n"))
            .expect("write /proc/self/uid_map");
        // setgroups must be denied before an unprivileged gid_map write
        // will be accepted (user_namespaces(7)). An absent
        // /proc/self/setgroups means setgroups(2) is already permanently
        // denied in an ancestor user namespace, so the gid_map
        // precondition is already satisfied — not an environment skip.
        match write_procfs("/proc/self/setgroups", "deny") {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("write /proc/self/setgroups: {err}"),
        }
        write_procfs("/proc/self/gid_map", &std::format!("0 {gid} 1\n"))
            .expect("write /proc/self/gid_map");

        // Make the root mount private recursively so tmpfs mounts inside
        // this namespace don't propagate back to the host.
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_REC | MsFlags::MS_PRIVATE,
            None::<&str>,
        )
        .expect("mount --make-rprivate /");

        // Remount /dev/shm as a 1 MiB tmpfs. The size= option is honored by
        // tmpfs and enforced at page-fault time: accesses to pages the
        // tmpfs can't back raise SIGBUS.
        mount(
            Some("tmpfs"),
            "/dev/shm",
            Some("tmpfs"),
            MsFlags::empty(),
            Some("size=1m"),
        )
        .expect("mount tmpfs size=1m at /dev/shm");
    }
}

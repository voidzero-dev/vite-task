//! Fast mpsc IPC channel implementation based on shared memory.

mod shm_io;

use std::{env::temp_dir, ffi::OsStr, fs::File, io, ops::Deref, path::PathBuf};

use allocator_api2::alloc::Global;
use fspy_nostd::Fat;
use fspy_nostd_alloc::OsCString;
use fspy_shm::Mapping;
pub use shm_io::FrameMut;
use shm_io::{ShmReader, ShmWriter};
use tracing::debug;
use uuid::Uuid;
use wincode::{SchemaRead, SchemaWrite};

use super::IpcStr;

/// Prefix of shared-memory backing file names inside the system temporary
/// directory.
///
/// The files sit directly in the temporary directory. A shared subdirectory
/// would belong to whichever user created it first and block everyone else;
/// uniquely named `0o600` files in the sticky-bit temp directory avoid that.
const SHM_BACKING_PREFIX: &str = "vite-task-fspy-";

/// Serializable configuration to create channel senders.
#[derive(SchemaWrite, SchemaRead, Clone, Debug)]
pub struct ChannelConf {
    lock_file_path: Box<IpcStr>,
    shm_id: Box<IpcStr>,
}

/// Creates a mpsc IPC channel with one receiver and a `ChannelConf` that can be passed around processes and used to create multiple senders
#[expect(clippy::missing_errors_doc, reason = "non-vt crate: cannot use vt_str/vt_path types")]
pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    // Initialize the lock file with a unique name.
    let lock_file_path = temp_dir().join(format!("fspy_ipc_{}.lock", Uuid::new_v4()));

    let shm_path = shm_backing_path()?;
    let shm_c_path = os_c_string(shm_path.as_os_str())?;
    let handle =
        fspy_shm::create(shm_c_path.as_c_str().as_thin(), capacity).map_err(shm_error_to_io)?;
    // The keeper exists from here on, so every error path below cleans up.
    let keeper = ShmKeeper { path: shm_c_path };
    let mapping = handle.map().map_err(shm_error_to_io)?;

    let conf = ChannelConf {
        lock_file_path: lock_file_path.as_os_str().into(),
        shm_id: shm_path.as_os_str().into(),
    };

    let receiver = Receiver::new(lock_file_path, keeper, mapping)?;
    Ok((conf, receiver))
}

/// Encodes `path` as an owned NUL-terminated platform C string.
fn os_c_string(path: &OsStr) -> io::Result<OsCString<Fat, Global>> {
    let mut units = os_units(path);
    units.push(0);
    OsCString::from_vec_with_nul(units)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

#[cfg(unix)]
fn os_units(path: &OsStr) -> allocator_api2::vec::Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;

    let mut units = allocator_api2::vec::Vec::with_capacity(path.len() + 1);
    units.extend_from_slice(path.as_bytes());
    units
}

#[cfg(windows)]
fn os_units(path: &OsStr) -> allocator_api2::vec::Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;

    let mut units = allocator_api2::vec::Vec::with_capacity(path.len() + 1);
    for unit in path.encode_wide() {
        units.push(unit);
    }
    units
}

#[cfg(unix)]
fn shm_error_to_io(error: fspy_nostd::Error) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(windows)]
fn shm_error_to_io(error: fspy_nostd::Error) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error().cast_signed())
}

/// Returns a fresh absolute path for a shared-memory backing file.
fn shm_backing_path() -> io::Result<PathBuf> {
    // `temp_dir` reflects `TMPDIR` verbatim, which may be relative. The path
    // travels to processes with other working directories, so resolve it
    // against the creator's current directory first.
    let path = std::path::absolute(temp_dir())?
        .join(format!("{SHM_BACKING_PREFIX}{}.shm", Uuid::new_v4().simple()));
    #[cfg(windows)]
    let path = to_verbatim_if_long(path)?;
    Ok(path)
}

/// Converts long paths to verbatim (`\\?\`) form up front, so every later use
/// of the path — creation here, opening in any process, removal — stays clear
/// of the legacy `MAX_PATH` limit without relying on the system's long-path
/// opt-in, which the arbitrary processes opening shared memory could not
/// count on anyway.
#[cfg(windows)]
fn to_verbatim_if_long(path: PathBuf) -> io::Result<PathBuf> {
    use std::os::windows::ffi::OsStrExt as _;

    use omnipath::windows::WinPathExt as _;

    // The length at which std's own Windows path conversion switches to a
    // verbatim path.
    const VERBATIM_THRESHOLD: usize = 248;

    if path.as_os_str().encode_wide().count() >= VERBATIM_THRESHOLD {
        return path.to_verbatim();
    }
    Ok(path)
}

/// Keeps the shared memory's backing path alive and removes it on drop.
///
/// Removal is cleanup, not a stop signal: later opens fail, but existing
/// handles and mappings keep reading and writing; see [`fspy_shm::remove`].
struct ShmKeeper {
    path: OsCString<Fat, Global>,
}

impl Drop for ShmKeeper {
    fn drop(&mut self) {
        let _ = fspy_shm::remove(self.path.as_c_str().as_thin());
    }
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

        let shm_path = os_c_string(&self.shm_id.to_cow_os_str())?;
        let mapping = fspy_shm::open(shm_path.as_c_str().as_thin())
            .map_err(shm_error_to_io)?
            .map()
            .map_err(shm_error_to_io)?;
        // SAFETY: `mapping` is a freshly mapped shared memory region with valid
        // pointer and size. Exclusive write access is ensured by the shared
        // file lock held by this sender.
        let writer = unsafe { ShmWriter::new(mapping) };
        Ok(Sender { writer, lock_file, lock_file_path: self.lock_file_path.clone() })
    }
}

pub struct Sender {
    writer: ShmWriter<Mapping>,
    lock_file_path: Box<IpcStr>,
    lock_file: File,
}

impl Drop for Sender {
    fn drop(&mut self) {
        if let Err(err) = self.lock_file.unlock() {
            let lock_file_path = self.lock_file_path.to_cow_os_str();
            debug!("Failed to unlock the shared IPC lock {}: {}", lock_file_path.display(), err);
        }
    }
}

impl Deref for Sender {
    type Target = ShmWriter<Mapping>;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}

/// SAFETY: `Sender` holds a shared file lock that ensures there's no reader, so `shm` can be safely written to.
unsafe impl Send for Sender {}

/// SAFETY: `Sender` holds a shared file lock that ensures there's no reader, so `shm` can be safely written to.
unsafe impl Sync for Sender {}

/// The unique receiver side of an IPC channel.
/// Owns the lock file and removes it on drop.
pub struct Receiver {
    lock_file_path: PathBuf,
    lock_file: File,
    /// Keeps the shared memory's backing file alive for as long as senders
    /// may attach.
    _keeper: ShmKeeper,
    mapping: Mapping,
}

/// SAFETY: `Receiver` doesn't read or write `shm`. It only passes it to `ReceiverLockGuard` under the lock.
unsafe impl Send for Receiver {}

/// SAFETY: `Receiver` doesn't read or write `shm`. It only passes it to `ReceiverLockGuard` under the lock.
unsafe impl Sync for Receiver {}

impl Drop for Receiver {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_file_path) {
            debug!("Failed to remove IPC lock file {}: {}", self.lock_file_path.display(), err);
        }
    }
}

impl Receiver {
    fn new(lock_file_path: PathBuf, keeper: ShmKeeper, mapping: Mapping) -> io::Result<Self> {
        let lock_file = File::create(&lock_file_path)?;
        Ok(Self { lock_file_path, lock_file, _keeper: keeper, mapping })
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
        let reader = ShmReader::new(unsafe { self.mapping.as_slice() });
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
    use std::{ffi::OsString, fs, num::NonZeroUsize, str::from_utf8};

    use bstr::B;
    use subprocess_test::command_for_fn;

    use super::*;

    /// The shared-memory path is generated absolute, so a sender in a process
    /// with a different working directory and a relative temporary directory
    /// must still attach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_ignores_changed_temp_and_working_directory() {
        let (conf, receiver) = channel(100).unwrap();
        let changed_cwd = temp_dir().join(format!("fspy-ipc-changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();

        let mut command = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
        });
        command.cwd = changed_cwd.clone();
        for name in ["TMPDIR", "TMP", "TEMP"] {
            command.envs.insert(OsString::from(name), OsString::from("changed-relative-tmp"));
        }
        let succeeded = std::process::Command::from(command).status().unwrap().success();
        fs::remove_dir(changed_cwd).unwrap();
        assert!(succeeded);

        let lock = receiver.lock().unwrap();
        assert_eq!(lock.iter_frames().next().unwrap(), &[4, 2]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn smoke() {
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_locked() {
        let (conf, receiver) = channel(42).unwrap();
        let _lock = receiver.lock().unwrap();

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert_eq!(B(&output.stdout), B("false"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_receiver_dropped() {
        let (conf, receiver) = channel(42).unwrap();
        drop(receiver);

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert_eq!(B(&output.stdout), B("false"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_senders() {
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

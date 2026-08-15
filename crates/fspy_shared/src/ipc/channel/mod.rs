//! Fast mpsc IPC channel implementation based on shared memory.
//!
//! The channel is crash-tolerant and nonblocking on both ends: any sender
//! process may die (or keep running) at any point without preventing the
//! receiver from closing the channel and reading every committed frame. See
//! the `shm_io` module for the underlying protocol.

mod shm_io;

use std::{env::temp_dir, ffi::OsStr, io, ops::Deref, path::PathBuf};

use allocator_api2::alloc::Global;
use fspy_nostd::Fat;
use fspy_nostd_alloc::OsCString;
use fspy_shm::Mapping;
use shm_io::ShmWriter;
pub use shm_io::{ClaimError, FrameMut, WriteEncodedError};

/// The committed frames of a closed channel; borrows the shared mapping,
/// which stays alive (and mapped) until this value drops.
pub type Frames = shm_io::Frames<Mapping>;
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
    shm_id: Box<IpcStr>,
}

/// Creates a mpsc IPC channel with one receiver and a `ChannelConf` that can be passed around processes and used to create multiple senders.
///
/// The channel's layout is derived from `capacity` alone, on both ends, so
/// senders need no configuration beyond the `ChannelConf`.
#[expect(clippy::missing_errors_doc, reason = "non-vt crate: cannot use vt_str/vt_path types")]
pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    let shm_c_path = os_c_string(shm_backing_path()?.as_os_str())?;
    let handle =
        fspy_shm::create(shm_c_path.as_c_str().as_thin(), capacity).map_err(shm_error_to_io)?;
    // The keeper exists from here on, so every error path below cleans up.
    let keeper = ShmKeeper { path: shm_c_path };
    let mapping = handle.map().map_err(shm_error_to_io)?;

    // On Linux, the first touch of the sparse backing file — read or write —
    // can cost milliseconds of journalled first-block allocation, and it
    // would otherwise be paid by a sender's first record or by close's
    // snapshot. Touch the header page through a second view concurrently
    // with process startup instead. Only there: on Windows and macOS the
    // first touch is cheap and the extra thread costs more than it saves.
    // Best-effort — a channel without the pre-fault is merely slower.
    #[cfg(target_os = "linux")]
    if let Ok(prefault_mapping) = handle.map() {
        let _ = std::thread::Builder::new().name("fspy-shm-prefault".into()).spawn(move || {
            // SAFETY: the mapping views the region created zero-initialized
            // above, which is only accessed through the `shm_io` protocol.
            unsafe { shm_io::pre_fault(&prefault_mapping) };
        });
    }

    let conf = ChannelConf { shm_id: IpcStr::from_os_c_str(keeper.path.as_c_str()).to_boxed() };

    Ok((conf, Receiver { _keeper: keeper, mapping }))
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
    /// Never blocks. Fails when the receiver has already closed the channel
    /// or dropped: the backing file is then removed (and, for the removal
    /// failure edge, the region itself is marked closed).
    #[expect(
        clippy::missing_errors_doc,
        reason = "error conditions are self-evident from return type"
    )]
    pub fn sender(&self) -> io::Result<Sender> {
        // The arena never touches the process heap, so this stays safe in
        // the preload contexts that create senders (pre-`main` constructors,
        // the Windows loader lock).
        let arena = fspy_nostd_alloc::arena();
        let shm_path = self.shm_id.to_os_c_string_in(&arena).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid shared-memory path")
        })?;
        let mapping = fspy_shm::open(shm_path.as_c_str().as_thin())
            .map_err(shm_error_to_io)?
            .map()
            .map_err(shm_error_to_io)?;
        // A truncated or foreign file must fail here, not panic the host
        // process inside the protocol's geometry assertions.
        if !shm_io::is_supported_region_len(mapping.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared-memory region size cannot host the channel",
            ));
        }
        // SAFETY: `mapping` is a freshly mapped shared memory region created
        // zero-initialized by `channel` and accessed only through the
        // `shm_io` protocol by every attached process; the protocol derives
        // one layout from the mapped size on every side.
        let writer = unsafe { ShmWriter::new(mapping) };
        if writer.is_closed() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the channel has been closed by the receiver",
            ));
        }
        Ok(Sender { writer })
    }
}

pub struct Sender {
    writer: ShmWriter<Mapping>,
}

impl Deref for Sender {
    type Target = ShmWriter<Mapping>;

    fn deref(&self) -> &Self::Target {
        &self.writer
    }
}

// SAFETY: `Sender` only accesses the shared mapping through the `shm_io`
// protocol, which synchronizes concurrent writers and the receiver with
// atomic operations; the mapping's address is stable and independently owned.
unsafe impl Send for Sender {}

// SAFETY: see the `Send` impl; `ShmWriter`'s shared-reference API is
// internally synchronized by the protocol.
unsafe impl Sync for Sender {}

/// The unique receiver side of an IPC channel.
///
/// Holds the shared memory and its backing file alive for as long as senders
/// may attach; [`Receiver::close`] (or dropping) removes the backing file.
pub struct Receiver {
    /// Keeps the shared memory's backing file alive for as long as senders
    /// may attach.
    _keeper: ShmKeeper,
    mapping: Mapping,
}

// SAFETY: `Receiver` only holds the mapping; it accesses it exclusively
// through the `shm_io` protocol in `close`, which synchronizes with senders
// via atomic operations. The mapping's address is stable and independently
// owned.
unsafe impl Send for Receiver {}

// SAFETY: see the `Send` impl.
unsafe impl Sync for Receiver {}

impl Receiver {
    /// Closes the channel and returns every committed frame, borrowed from
    /// the shared mapping that moves into the returned [`Frames`].
    ///
    /// Never blocks on senders: new claims are rejected from this point on,
    /// unfinished frames are atomically aborted, and committed frames become
    /// readable in place. A sender process that is still alive keeps
    /// running; anything it reports after this point is outside the
    /// channel's boundary by design. The mapping is released when the
    /// returned [`Frames`] drops.
    ///
    /// # Errors
    ///
    /// Fails only when the shared-memory metadata was corrupted (a protocol
    /// impossibility for correct senders); the trace is then unusable.
    pub fn close(self) -> io::Result<Frames> {
        let Self { _keeper: keeper, mapping } = self;
        // Remove the backing file first so no new process attaches while the
        // channel closes.
        drop(keeper);
        // SAFETY: `mapping` was created zero-initialized by `channel`, its
        // address is stable, and all attached processes access it only
        // through the `shm_io` protocol.
        unsafe { shm_io::close(mapping) }
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, num::NonZeroUsize, str::from_utf8};

    use assert2::assert;
    use bstr::B;
    use subprocess_test::command_for_fn;

    use super::*;

    /// The shared-memory path is generated absolute, so a sender in a process
    /// with a different working directory and a relative temporary directory
    /// must still attach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_ignores_changed_temp_and_working_directory() {
        let (conf, receiver) = channel(4096).unwrap();
        let changed_cwd = temp_dir().join(format!("fspy-ipc-changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();

        let mut command = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
            frame.finish();
        });
        command.cwd = changed_cwd.clone();
        for name in ["TMPDIR", "TMP", "TEMP"] {
            command.envs.insert(OsString::from(name), OsString::from("changed-relative-tmp"));
        }
        let succeeded = std::process::Command::from(command).status().unwrap().success();
        fs::remove_dir(changed_cwd).unwrap();
        assert!(succeeded);

        let frames = receiver.close().unwrap();
        assert!(frames.iter().next().unwrap() == &[4, 2]);
        assert!(frames.is_complete());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn smoke() {
        let (conf, receiver) = channel(4096).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
            frame.finish();
        });
        assert!(std::process::Command::from(cmd).status().unwrap().success());

        let frames = receiver.close().unwrap();
        let mut iter = frames.iter();

        let received_frame = iter.next().unwrap();
        assert!(received_frame == &[4, 2]);

        assert!(iter.next().is_none());
        assert!(frames.is_complete());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_close() {
        let (conf, receiver) = channel(4096).unwrap();
        let _frames = receiver.close().unwrap();

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert!(B(&output.stdout) == B("false"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_receiver_dropped() {
        let (conf, receiver) = channel(4096).unwrap();
        drop(receiver);

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert!(B(&output.stdout) == B("false"));
    }

    /// A sender that attached before close keeps its mapping but cannot
    /// claim any new frame afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attached_sender_cannot_claim_after_close() {
        let (conf, receiver) = channel(4096).unwrap();
        let sender = conf.sender().unwrap();

        let mut frame = sender.claim_frame(NonZeroUsize::new(2).unwrap()).unwrap();
        frame.copy_from_slice(&[4, 2]);
        frame.finish();

        let frames = receiver.close().unwrap();
        assert!(frames.iter().next().unwrap() == &[4, 2]);
        assert!(frames.is_complete());

        assert!(
            sender.claim_frame(NonZeroUsize::new(2).unwrap()).unwrap_err() == ClaimError::Closed
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_senders() {
        // 64 KiB: a 1023-slot table for the 200 frames sent below.
        let (conf, receiver) = channel(64 * 1024).unwrap();
        for i in 0u16..200 {
            let cmd = command_for_fn!((conf.clone(), i), |(conf, i): (ChannelConf, u16)| {
                let sender = conf.sender().unwrap();
                let data_to_send = i.to_string();
                let mut frame =
                    sender.claim_frame(NonZeroUsize::new(data_to_send.len()).unwrap()).unwrap();
                frame.copy_from_slice(data_to_send.as_bytes());
                frame.finish();
            });
            let output = std::process::Command::from(cmd).output().unwrap();
            assert!(
                output.status.success(),
                "Failed to send in iteration {}: {:?}",
                i,
                B(&output.stderr)
            );
        }
        let frames = receiver.close().unwrap();
        let mut received_values: Vec<u16> =
            frames.iter().map(|frame| from_utf8(frame).unwrap().parse::<u16>().unwrap()).collect();
        received_values.sort_unstable();
        assert!(received_values == (0u16..200).collect::<Vec<u16>>());
        assert!(frames.is_complete());
    }
}

//! Fast mpsc IPC channel implementation based on shared memory.
//!
//! The channel is crash-tolerant and nonblocking on both ends: any sender
//! process may die (or keep running) at any point without preventing the
//! receiver from closing the channel and reading every committed frame. See
//! the `shm_io` module for the underlying protocol.

mod shm_io;

use std::{env::temp_dir, ffi::OsStr, io, num::NonZeroUsize, path::PathBuf};

use allocator_api2::alloc::Global;
use fspy_nostd::Fat;
use fspy_nostd_alloc::OsCString;
use fspy_shm::Mapping;
use shm_io::{ShmReader, ShmWriter};

/// Descriptor slots per channel — the compile-time half of the region's
/// shape, shared by the receiver and every sender through this constant.
/// It matches what the old an-eighth-of-the-region rule gave the 4 GiB
/// production region: one 8-byte descriptor per ~56 payload bytes at full
/// capacity, generous slack for record-sized frames. The table is sparse
/// address space until slots are actually touched.
const SLOTS: usize = 1 << 26;

/// Reads the committed frames of a sealed channel; borrows the shared
/// mapping, which stays alive (and mapped) until this value drops.
pub type FrameReader = shm_io::ShmReader<Mapping>;
use uuid::Uuid;
use wincode::{SchemaRead, SchemaWrite, Serialize as _, config::DefaultConfig};

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
/// `capacity` is the region size in bytes; it must hold the compile-time
/// descriptor table, and the rest of it is payload room. Senders need no
/// configuration beyond the `ChannelConf` — the layout is the
/// compile-time table plus whatever the mapped file's size says.
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
            unsafe { shm_io::pre_fault::<SLOTS>(&prefault_mapping) };
        });
    }

    // Prove the region can host the protocol — the same fallible attach
    // senders perform — so a bad capacity fails the task now, not at its
    // first record.
    // SAFETY: the region was just created zero-initialized and is only
    // accessed through the `shm_io` protocol.
    if unsafe { ShmWriter::<_, SLOTS>::new(&mapping) }.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capacity cannot host the channel's descriptor table",
        ));
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
        // SAFETY: `mapping` is a freshly mapped shared memory region created
        // zero-initialized by `channel` and accessed only through the
        // `shm_io` protocol by every attached process.
        let Some(writer) = (unsafe { ShmWriter::new(mapping) }) else {
            // A truncated or foreign file fails here — it never panics the
            // host process.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared-memory region cannot host the channel",
            ));
        };
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
    writer: ShmWriter<Mapping, SLOTS>,
}

impl Sender {
    /// Serializes one record into a committed frame.
    ///
    /// A record that cannot be sent is skipped, because that is all a
    /// sender inside an intercepted call can do: the channel may have
    /// closed (the record belongs past its boundary), or the region may be
    /// full (a loss the failed claim reports by setting the CLOSED gate,
    /// so the receiver reports the frames incomplete).
    pub fn send<T: SchemaWrite<DefaultConfig, Src = T>>(&self, value: &T) {
        let Ok(serialized_size) = T::serialized_size(value) else {
            return;
        };
        let Ok(Some(frame_size)) = usize::try_from(serialized_size).map(NonZeroUsize::new) else {
            return;
        };
        let Ok(mut frame) = self.writer.claim_frame(frame_size) else {
            return;
        };
        let mut buf: &mut [u8] = &mut frame;
        if T::serialize_into(&mut buf, value).is_err() || !buf.is_empty() {
            // An abandoned frame; the receiver ignores its slot.
            return;
        }
        frame.finish();
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
    /// the shared mapping that moves into the returned [`FrameReader`].
    ///
    /// Never blocks on senders: new claims are rejected from this point on,
    /// unfinished frames are atomically aborted, and committed frames become
    /// readable in place. A sender process that is still alive keeps
    /// running; anything it reports after this point is outside the
    /// channel's boundary by design. The mapping is released when the
    /// returned [`FrameReader`] drops.
    ///
    /// # Errors
    ///
    /// Fails when a record was lost before the close — a sender ran out of
    /// room, or tried to send something too large — and when the region
    /// cannot hold the protocol at all. Either way there is no complete
    /// set of records, so none are handed back.
    pub fn close(self) -> io::Result<FrameReader> {
        let Self { _keeper: keeper, mapping } = self;
        // Remove the backing file first so no new process attaches while the
        // channel closes.
        drop(keeper);
        // SAFETY: `mapping` was created zero-initialized by `channel`, its
        // address is stable and independently owned, and all attached
        // processes access it only through the `shm_io` protocol.
        unsafe { ShmReader::seal::<SLOTS>(mapping) }
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

    /// Any test region must hold the compile-time table; sparse, so cheap.
    const GIB: usize = 1 << 30;

    /// The shared-memory path is generated absolute, so a sender in a process
    /// with a different working directory and a relative temporary directory
    /// must still attach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_ignores_changed_temp_and_working_directory() {
        let (conf, receiver) = channel(GIB).unwrap();
        let changed_cwd = temp_dir().join(format!("fspy-ipc-changed-cwd-{}", Uuid::new_v4()));
        fs::create_dir(&changed_cwd).unwrap();

        let mut command = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.writer.claim_frame(frame_size).unwrap();
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
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn smoke() {
        let (conf, receiver) = channel(GIB).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.writer.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
            frame.finish();
        });
        assert!(std::process::Command::from(cmd).status().unwrap().success());

        let frames = receiver.close().unwrap();
        let mut iter = frames.iter();

        let received_frame = iter.next().unwrap();
        assert!(received_frame == &[4, 2]);

        assert!(iter.next().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_close() {
        let (conf, receiver) = channel(GIB).unwrap();
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
        let (conf, receiver) = channel(GIB).unwrap();
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
        let (conf, receiver) = channel(GIB).unwrap();
        let sender = conf.sender().unwrap();

        let mut frame = sender.writer.claim_frame(NonZeroUsize::new(2).unwrap()).unwrap();
        frame.copy_from_slice(&[4, 2]);
        frame.finish();

        let frames = receiver.close().unwrap();
        assert!(frames.iter().next().unwrap() == &[4, 2]);

        assert!(
            sender.writer.claim_frame(NonZeroUsize::new(2).unwrap()).unwrap_err()
                == shm_io::ClaimError::Closed
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_senders() {
        let (conf, receiver) = channel(GIB).unwrap();
        for i in 0u16..200 {
            let cmd = command_for_fn!((conf.clone(), i), |(conf, i): (ChannelConf, u16)| {
                let sender = conf.sender().unwrap();
                let data_to_send = i.to_string();
                let mut frame = sender
                    .writer
                    .claim_frame(NonZeroUsize::new(data_to_send.len()).unwrap())
                    .unwrap();
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
    }
}

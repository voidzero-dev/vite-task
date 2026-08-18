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
use shm_io::{SealError, ShmReader, ShmWriter};

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

/// Descriptor slots in every channel, one per record.
///
/// Fixed here so both ends agree on it without carrying it between them.
/// At the 4 GiB a tracked run gets it is one 8-byte slot per 56 payload
/// bytes, and records run a few hundred bytes each, so payload space runs
/// out long before slots do. The table is sparse address space until the
/// slots are claimed, so a channel pays only for the ones it uses.
const SLOTS: usize = 1 << 26;

/// Serializable configuration to create channel senders.
#[derive(SchemaWrite, SchemaRead, Clone, Debug)]
pub struct ChannelConf {
    shm_id: Box<IpcStr>,
}

/// Creates a mpsc IPC channel with one receiver and a `ChannelConf` that can be passed around processes and used to create multiple senders.
#[expect(clippy::missing_errors_doc, reason = "non-vt crate: cannot use vt_str/vt_path types")]
pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    let shm_c_path = os_c_string(shm_backing_path()?.as_os_str())?;
    let handle =
        fspy_shm::create(shm_c_path.as_c_str().as_thin(), capacity).map_err(shm_error_to_io)?;
    // The keeper exists from here on, so every error path below cleans up.
    let keeper = ShmKeeper { path: shm_c_path };
    let mapping = handle.map().map_err(shm_error_to_io)?;

    // Prove the region can host the protocol — the same fallible attach
    // senders perform — so a size the two halves do not fit in fails the
    // task now, not at its first record.
    // SAFETY: the region was just created zero-initialized and is only
    // accessed through the `shm_io` protocol.
    if unsafe { ShmWriter::new(&mapping, SLOTS) }.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the shared-memory capacity has no room for the descriptor table",
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
    /// Creates a sender, or `None` when the channel is already over.
    ///
    /// Never blocks. `None` means the receiver removed the backing file,
    /// or sealed the region before removing it and this call caught the
    /// gate in between. Either way whatever the caller does next happens
    /// past the receiver's boundary, so recording nothing loses nothing.
    ///
    /// # Panics
    ///
    /// When the channel is there but cannot be attached to: its path is
    /// unreadable, the file refuses to open or map, or the region cannot
    /// hold the protocol. A process with no sender has no way to tell the
    /// receiver it recorded nothing, and a trace that silently omits every
    /// access a process made is worse than no trace, so it stops here.
    #[must_use]
    pub fn sender(&self) -> Option<Sender> {
        // The arena never touches the process heap, so this stays safe in
        // the preload contexts that create senders (pre-`main` constructors,
        // the Windows loader lock).
        let arena = fspy_nostd_alloc::arena();
        let shm_path = self
            .shm_id
            .to_os_c_string_in(&arena)
            .expect("the channel's shared-memory path is not a valid C string");
        let mapping = match fspy_shm::open(shm_path.as_c_str().as_thin()) {
            Ok(handle) => handle.map().expect("cannot map the shared-memory channel"),
            Err(error) => {
                let error = shm_error_to_io(error);
                // The receiver removed the backing file, so it has already
                // stopped collecting.
                if error.kind() == io::ErrorKind::NotFound {
                    return None;
                }
                panic!("cannot open the shared-memory channel: {error}");
            }
        };
        // SAFETY: `mapping` is a freshly mapped shared memory region created
        // zero-initialized by `channel` and accessed only through the
        // `shm_io` protocol by every attached process.
        let writer = unsafe { ShmWriter::new(mapping, SLOTS) }
            .expect("the shared-memory region cannot hold the channel");
        // The receiver sealed the region but has not removed it yet.
        if writer.is_closed() {
            return None;
        }
        Some(Sender { writer })
    }
}

pub struct Sender {
    writer: ShmWriter<Mapping>,
}

impl Sender {
    /// Serializes one record into a committed frame.
    ///
    /// A claim the channel refuses is skipped, because that is all a sender
    /// inside an intercepted call can do: the channel has closed, so the
    /// record belongs past the receiver's boundary, or the region is full
    /// and the failed claim already set the CLOSED gate to say so.
    ///
    /// # Panics
    ///
    /// When the record's serialized size disagrees with the bytes it then
    /// writes. Nothing the caller passes can cause that, so it is a defect
    /// in this crate or its codec, and a trace built on it would be wrong
    /// in ways the receiver cannot see.
    pub fn send<T: SchemaWrite<DefaultConfig, Src = T>>(&self, value: &T) {
        let serialized_size =
            T::serialized_size(value).expect("a record cannot report its serialized size");
        let frame_size = usize::try_from(serialized_size)
            .ok()
            .and_then(NonZeroUsize::new)
            .expect("a record reports a serialized size of zero, or one no frame could hold");
        let Ok(mut frame) = self.writer.claim_frame(frame_size) else {
            return;
        };
        let mut buf: &mut [u8] = &mut frame;
        T::serialize_into(&mut buf, value).expect("a record will not serialize into its own frame");
        assert!(buf.is_empty(), "a record wrote fewer bytes than the size it reported");
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
    /// Never blocks on senders: it reads the claim counter once, which
    /// fixes how far reading goes, and shuts the gate so no later claim
    /// succeeds. Committed frames become readable in place. A sender that
    /// was still filling a frame keeps running, and its frame may or may
    /// not appear depending on whether it commits before the read reaches
    /// that slot — either way the operation it describes happens after the
    /// receiver stopped collecting. The mapping is released when the
    /// returned [`FrameReader`] drops.
    ///
    /// # Errors
    ///
    /// [`RecordsLost`] when a sender could not record something it went on
    /// to do. There is no complete set of records then, so none are handed
    /// back, and a caller that needs the trace has to treat the run as
    /// untracked rather than as having reported nothing.
    ///
    /// # Panics
    ///
    /// When the region cannot hold the protocol, which [`channel`] proved
    /// it could before any sender saw it.
    pub fn close(self) -> Result<FrameReader, RecordsLost> {
        let Self { _keeper: keeper, mapping } = self;
        // SAFETY: `mapping` was created zero-initialized by `channel`, its
        // address is stable and independently owned, and all attached
        // processes access it only through the `shm_io` protocol.
        let sealed = unsafe { ShmReader::seal(mapping, SLOTS) };
        // Remove the backing file only after the gate is shut. A process
        // that attaches in between finds a closed channel and gives up
        // cleanly; one that found the file already gone could not attach at
        // all, and so could not report whatever it then failed to record.
        drop(keeper);
        match sealed {
            Ok(reader) => Ok(reader),
            // This receiver is the only one that could have sealed, and it
            // is gone by now, so the gate can only be a sender's report.
            Err(SealError::Closed) => Err(RecordsLost),
            Err(SealError::UnsupportedRegion) => {
                panic!("the shared-memory region cannot hold the channel")
            }
        }
    }
}

/// A sender could not record something, so the receiver has no complete
/// set of records to hand back.
///
/// The region filled up, or a record came out longer than one frame can
/// hold. Both are the channel running out of room rather than anything the
/// senders did wrong, and both leave the run untracked.
#[derive(thiserror::Error, Clone, Copy, PartialEq, Eq, Debug)]
#[error("a sender ran out of room in the shared-memory channel")]
pub struct RecordsLost;

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, fs, num::NonZeroUsize, str::from_utf8};

    use assert2::assert;
    use bstr::B;
    use subprocess_test::command_for_fn;

    use super::*;
    use crate::ipc::{AccessMode, IpcPath, PathAccess};

    /// A gibibyte, which clears the table's half of sparse address space
    /// and leaves the rest for payloads. None of it costs real memory
    /// until something is written there.
    const CAPACITY: usize = 1 << 30;

    /// A capacity with no room for the table has to fail here, at
    /// creation. Everything downstream treats the region as able to host
    /// the protocol: `sender` panics when it cannot, and so does
    /// `Receiver::close`.
    #[test]
    fn a_capacity_too_small_for_the_table_fails_the_channel() {
        // The counters alone need sixteen bytes, and the table needs eight
        // per slot on top.
        let Err(error) = channel(8) else {
            panic!("a region too small for the protocol made a channel");
        };
        assert!(error.kind() == io::ErrorKind::InvalidInput);
    }

    /// The shared-memory path is generated absolute, so a sender in a process
    /// with a different working directory and a relative temporary directory
    /// must still attach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_ignores_changed_temp_and_working_directory() {
        let (conf, receiver) = channel(CAPACITY).unwrap();
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

    /// `Sender::send` is the only writer production uses, and the rest of
    /// these tests reach past it into `claim_frame`. This one drives it
    /// end to end, so that a disagreement between `serialized_size` and
    /// `serialize_into` — which would silently drop every record — fails
    /// here rather than in a build.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_round_trips_records() {
        let (conf, receiver) = channel(CAPACITY).unwrap();
        let sender = conf.sender().unwrap();
        // A record path carries the platform's own string form: bytes on
        // unix, UTF-16 on Windows.
        #[cfg(unix)]
        let owned = ["/tmp/one", "/tmp/two/three"];
        #[cfg(windows)]
        let owned = [r"C:\tmp\one", r"C:\tmp\two\three"]
            .map(|path| path.encode_utf16().collect::<Vec<_>>());
        #[cfg(unix)]
        let paths = owned.map(<&IpcPath>::from);
        #[cfg(windows)]
        let paths = [IpcPath::from_wide(&owned[0]), IpcPath::from_wide(&owned[1])];

        for path in paths {
            sender.send(&PathAccess::read(path));
        }
        drop(sender);

        let frames = receiver.close().unwrap();
        let mut iter = frames.iter();
        for path in paths {
            let access: PathAccess<'_> = wincode::deserialize_exact(iter.next().unwrap()).unwrap();
            assert!(access.path == path);
            assert!(access.mode == AccessMode::READ);
        }
        assert!(iter.next().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn smoke() {
        let (conf, receiver) = channel(CAPACITY).unwrap();
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
        let (conf, receiver) = channel(CAPACITY).unwrap();
        let _frames = receiver.close().unwrap();

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_some());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert!(B(&output.stdout) == B("false"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    async fn forbid_new_senders_after_receiver_dropped() {
        let (conf, receiver) = channel(CAPACITY).unwrap();
        drop(receiver);

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_some());
        });
        let output = std::process::Command::from(cmd).output().unwrap();
        assert!(B(&output.stdout) == B("false"));
    }

    /// A sender that attached before close keeps its mapping but cannot
    /// claim any new frame afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attached_sender_cannot_claim_after_close() {
        let (conf, receiver) = channel(CAPACITY).unwrap();
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
        let (conf, receiver) = channel(CAPACITY).unwrap();
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

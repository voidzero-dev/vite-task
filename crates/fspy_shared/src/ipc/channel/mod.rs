//! Fast mpsc IPC channel implementation based on shared memory.
//!
//! The shared memory carries its own close gate: a writer holds a gate guard
//! while it writes a frame, and the receiver closes the gate with one atomic
//! operation. The closed flag and the writer count live in the same atomic
//! word, so a process cannot write without being counted. The file lock this
//! replaces got that wrong: a tool that closed file descriptors it did not
//! recognize released the lock while it kept write access to the mapping.
//!
//! The close does not wait for anyone. Accesses made after the traced root
//! process exits are dropped by design, and a run that still had a write in
//! flight at that instant is reported as incomplete rather than read.

mod gate;
mod gated;

use std::io;

use fspy_shm::{Mapping, ShmKeeper};
pub use gated::{
    ClaimError, FrameMut, GatedShmReader, GatedShmReceiver, GatedShmWriter,
    StillWriting as TrackingIncomplete, WriteEncodedError,
};
use wincode::{SchemaRead, SchemaWrite};

use super::{NativeStr, PathAccess};

/// Serializable configuration to create channel senders.
#[derive(SchemaWrite, SchemaRead, Clone, Debug)]
pub struct ChannelConf {
    shm_id: Box<NativeStr>,
}

/// The write end of an IPC channel. Every frame is claimed through the gate, so
/// no writer can bypass it.
pub type Sender = GatedShmWriter<Mapping>;

/// The unique read end of an IPC channel, before it is closed.
///
/// Holds the shared memory's keeper: dropping the receiver, or closing it,
/// removes the backing file's name, after which no new sender can be created.
pub struct Receiver {
    keeper: ShmKeeper,
    inner: GatedShmReceiver<Mapping>,
}

impl Receiver {
    /// Closes the gate and freezes the region.
    ///
    /// Also drops the keeper: the attach window ends here, so a process that
    /// opens the channel later fails at `open` instead of at its first claim.
    ///
    /// # Errors
    ///
    /// Returns [`TrackingIncomplete`] when a write was still in flight at the
    /// instant of the close. The region is not read in that case.
    pub fn close(self) -> Result<ChannelFrames, TrackingIncomplete> {
        drop(self.keeper);
        self.inner.close()
    }
}

/// A frozen channel, handed out by [`Receiver::close`].
pub type ChannelFrames = GatedShmReader<Mapping>;

/// Creates a mpsc IPC channel with one receiver and a `ChannelConf` that can be passed around processes and used to create multiple senders
#[expect(clippy::missing_errors_doc, reason = "non-vt crate: cannot use vt_str/vt_path types")]
pub fn channel(capacity: usize) -> io::Result<(ChannelConf, Receiver)> {
    let (keeper, handle) = fspy_shm::create(capacity)?;
    let mapping = handle.map()?;
    let conf = ChannelConf { shm_id: keeper.id().into() };

    // SAFETY: the backing file was just created zero-initialized, its identifier
    // is not yet published to any other process, the mapping stays valid while
    // the receiver holds it, and `Sender` is the only other way to reach it.
    let inner = unsafe { GatedShmReceiver::new(mapping) };
    Ok((conf, Receiver { keeper, inner }))
}

impl ChannelConf {
    /// Creates a sender.
    ///
    /// Fails once the receiver has been closed or dropped, because both remove
    /// the backing file's name. A sender created before that keeps working, but
    /// its claims fail with [`ClaimError::Closed`] after the close.
    #[expect(
        clippy::missing_errors_doc,
        reason = "error conditions are self-evident from return type"
    )]
    pub fn sender(&self) -> io::Result<Sender> {
        let mapping = fspy_shm::open(&self.shm_id.to_cow_os_str())?.map()?;
        // SAFETY: the mapping was created by `channel` under the same contract,
        // it stays valid while this sender holds it, and every process reaches it
        // only through `Sender` or `Receiver`.
        Ok(unsafe { Sender::new(mapping) })
    }
}

impl ChannelFrames {
    /// Decodes every path access recorded before the close.
    ///
    /// # Panics
    ///
    /// Panics if a frame does not decode. The frames were written by this same
    /// binary against this same schema, so that is a bug worth hearing about
    /// rather than a recoverable condition.
    pub fn iter_path_accesses(&self) -> impl Iterator<Item = PathAccess<'_>> {
        // The frames were written by this same binary, so a decode failure is a
        // bug worth hearing about rather than a recoverable condition.
        self.iter_frames().map(|frame| wincode::deserialize_exact(frame).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        io::{BufRead, BufReader, Read, Write},
        num::NonZeroUsize,
        process::{Command, Stdio},
        str::from_utf8,
    };

    use bstr::B;
    use subprocess_test::command_for_fn;

    use super::*;

    /// Smaller than a page, but large enough for the gate header plus a few
    /// frames.
    const SMALL_CAPACITY: usize = 256;
    /// Frames the straggler grandchild writes after its parent has exited.
    const STRAGGLER_FRAMES: usize = 16;
    /// Marks the re-executed process as the straggler grandchild.
    const STRAGGLER_ENV: &str = "FSPY_IPC_TEST_STRAGGLER";
    /// Carries the child's own command line so it can re-execute itself.
    ///
    /// `subprocess_test` dispatches from a library constructor, and on Linux
    /// `std::env::args_os` is empty that early, so the arguments travel through
    /// the environment instead.
    const STRAGGLER_ARGV_ENV: &str = "FSPY_IPC_TEST_STRAGGLER_ARGV";

    #[test]
    fn smoke() {
        let (conf, receiver) = channel(SMALL_CAPACITY).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame_size = NonZeroUsize::new(2).unwrap();
            let mut frame = sender.claim_frame(frame_size).unwrap();
            frame.copy_from_slice(&[4, 2]);
        });
        assert!(Command::from(cmd).status().unwrap().success());

        let frames = receiver.close().unwrap();
        let mut frames = frames.iter_frames();

        assert_eq!(frames.next().unwrap(), &[4, 2]);
        assert!(frames.next().is_none());
    }

    #[test]
    fn claims_refused_after_close() {
        let (conf, receiver) = channel(SMALL_CAPACITY).unwrap();
        // A sender that attached while the channel was open.
        let sender = conf.sender().unwrap();
        let _frames = receiver.close().unwrap();

        // Its claims are refused after the close.
        let claim = sender.claim_frame(NonZeroUsize::new(2).unwrap());
        assert!(matches!(claim, Err(ClaimError::Closed)));

        // The close also dropped the keeper, so a process that starts later
        // does not even reach the shared memory.
        assert!(conf.sender().is_err());
    }

    #[test]
    #[expect(clippy::print_stdout, reason = "test diagnostics")]
    fn forbid_new_senders_after_receiver_dropped() {
        let (conf, receiver) = channel(SMALL_CAPACITY).unwrap();
        drop(receiver);

        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            print!("{}", conf.sender().is_ok());
        });
        let output = Command::from(cmd).output().unwrap();
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
            let output = Command::from(cmd).output().unwrap();
            assert!(
                output.status.success(),
                "Failed to send in iteration {}: {:?}",
                i,
                B(&output.stderr)
            );
        }
        let frames = receiver.close().unwrap();
        let mut received_values: Vec<u16> = frames
            .iter_frames()
            .map(|frame| from_utf8(frame).unwrap().parse::<u16>().unwrap())
            .collect();
        received_values.sort_unstable();
        assert_eq!(received_values, (0u16..200).collect::<Vec<u16>>());
    }

    /// The topology behind the descriptor-sweep bug: a child spawns a detached
    /// grandchild and exits, and the grandchild keeps writing. Closing must
    /// collect what the grandchild already wrote and must not wait for it to go
    /// away.
    #[test]
    fn straggler_writes_through_close() {
        let (conf, receiver) = channel(8192).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            if std::env::var_os(STRAGGLER_ENV).is_none() {
                // The child's only job is to leave a detached descendant behind
                // and exit. Re-executing itself keeps both roles in one closure.
                let argv = std::env::var(STRAGGLER_ARGV_ENV).unwrap();
                let mut grandchild = Command::new(std::env::current_exe().unwrap());
                grandchild.args(argv.split('\n')).env(STRAGGLER_ENV, "1");
                #[expect(
                    clippy::zombie_processes,
                    reason = "leaving the descendant behind unwaited is the situation under test"
                )]
                grandchild.spawn().unwrap();
                return;
            }

            let sender = conf.sender().unwrap();
            for i in 0..STRAGGLER_FRAMES {
                let data = i.to_string();
                sender
                    .claim_frame(NonZeroUsize::new(data.len()).unwrap())
                    .unwrap()
                    .copy_from_slice(data.as_bytes());
            }
            // Announce that every frame is written, then linger on the inherited
            // stdin until the test closes the channel and releases us.
            let mut stdout = std::io::stdout();
            stdout.write_all(b"written\n").unwrap();
            stdout.flush().unwrap();
            let mut ignored = Vec::new();
            std::io::stdin().read_to_end(&mut ignored).unwrap();
        });

        let mut cmd = cmd;
        let argv = cmd
            .args
            .iter()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect::<Vec<_>>()
            .join("\n");
        cmd.envs.insert(OsString::from(STRAGGLER_ARGV_ENV), OsString::from(argv));

        let mut child =
            Command::from(cmd).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        assert!(child.wait().unwrap().success());

        // The child is gone; the grandchild is still alive and holds both pipes.
        let mut ready = String::new();
        stdout.read_line(&mut ready).unwrap();
        assert_eq!(ready.trim_end(), "written");

        // Closing does not wait for the grandchild's lifetime, and everything it
        // wrote before the close is there.
        let frames = receiver.close().unwrap();
        let mut written: Vec<usize> = frames
            .iter_frames()
            .map(|frame| from_utf8(frame).unwrap().parse::<usize>().unwrap())
            .collect();
        written.sort_unstable();
        assert_eq!(written, (0..STRAGGLER_FRAMES).collect::<Vec<usize>>());

        // Closing the pipe lets the grandchild exit.
        drop(stdin);
    }

    /// A process killed between claiming a frame and finishing it leaks its gate
    /// count, which must fail closed rather than expose half-written memory.
    #[test]
    fn leaked_gate_guard_reports_incomplete() {
        let (conf, receiver) = channel(SMALL_CAPACITY).unwrap();
        let cmd = command_for_fn!(conf, |conf: ChannelConf| {
            let sender = conf.sender().unwrap();
            let frame = sender.claim_frame(NonZeroUsize::new(2).unwrap()).unwrap();
            // Stands in for a process dying mid-frame.
            std::mem::forget(frame);
        });
        assert!(Command::from(cmd).status().unwrap().success());

        let error = receiver.close().unwrap_err();
        assert_eq!(error.in_flight, 1);
    }
}

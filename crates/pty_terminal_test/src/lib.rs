use std::io::{BufRead, BufReader, Read};

pub use portable_pty::CommandBuilder;
use pty_terminal::terminal::{PtyReader, Terminal};
pub use pty_terminal::{
    ExitStatus,
    geo::ScreenSize,
    terminal::{ChildHandle, PtyWriter},
};

/// The OSC parameter ID that identifies milestone sequences.
const MILESTONE_OSC_ID: &[u8] = pty_terminal_test_client::MILESTONE_OSC_ID;

/// A test-oriented terminal that provides milestone-based synchronization.
///
/// Wraps a PTY terminal, splitting it into a [`PtyWriter`] for sending input
/// and a [`Reader`] that can wait for named milestones emitted by the child
/// process via [`pty_terminal_test_client::mark_milestone`].
pub struct TestTerminal {
    pub writer: PtyWriter,
    pub reader: Reader,
}

/// The read half of a test terminal, wrapping [`PtyReader`] with milestone support.
pub struct Reader {
    pty: BufReader<PtyReader>,
    child_handle: ChildHandle,
}

impl TestTerminal {
    /// Spawns a new child process in a test terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be opened or the command fails to spawn.
    pub fn spawn(size: ScreenSize, cmd: CommandBuilder) -> anyhow::Result<Self> {
        let Terminal { pty_reader, pty_writer, child_handle } = Terminal::spawn(size, cmd)?;
        Ok(Self {
            writer: pty_writer,
            reader: Reader { pty: BufReader::new(pty_reader), child_handle },
        })
    }
}

impl Reader {
    /// Reads from the PTY until a milestone with the given name is encountered.
    ///
    /// Returns the terminal screen contents at the moment the milestone is detected.
    ///
    /// Milestones use a uniform protocol across platforms: OSC marker followed
    /// by a non-visual DECSET 2026 on/off fence sequence.
    /// The fence is used as a stream delimiter so milestone parsing can detect
    /// marker boundaries without polluting screen contents.
    ///
    /// # Panics
    ///
    /// Panics if the child process exits (EOF) before the named milestone is received,
    /// or if a read error occurs.
    #[must_use]
    pub fn expect_milestone(&mut self, name: &str) -> String {
        let mut milestone = Vec::with_capacity(8 + MILESTONE_OSC_ID.len() + name.len());
        milestone.extend_from_slice(b"\x1b]");
        milestone.extend_from_slice(MILESTONE_OSC_ID);
        milestone.extend_from_slice(b";");
        milestone.extend_from_slice(name.as_bytes());
        milestone.push(0x07); // BEL terminator

        let fence = pty_terminal_test_client::MILESTONE_FENCE;
        let fence_last_byte = fence[fence.len() - 1];
        let mut buf = Vec::new();

        loop {
            let n = self.pty.read_until(fence_last_byte, &mut buf).expect("PTY read failed");
            assert!(n > 0, "EOF reached before milestone '{name}'");

            if buf.ends_with(fence) && buf.windows(milestone.len()).any(|w| w == milestone) {
                return self.pty.get_ref().screen_contents();
            }
        }
    }

    /// Reads all remaining PTY output until the child exits, then returns the exit status.
    ///
    /// # Panics
    ///
    /// Panics if reading from the PTY fails.
    pub fn wait_for_exit(&mut self) -> ExitStatus {
        let mut discard = Vec::new();
        self.pty.read_to_end(&mut discard).expect("PTY read_to_end failed");
        self.child_handle.wait()
    }
}

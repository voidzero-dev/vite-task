use std::io::Read;

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
    pty: PtyReader,
    child_handle: ChildHandle,
    cursor_hidden: bool,
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
            reader: Reader { pty: pty_reader, child_handle, cursor_hidden: false },
        })
    }
}

impl Reader {
    /// Reads from the PTY until a milestone with the given name is encountered.
    ///
    /// Returns the terminal screen contents at the moment the milestone is detected.
    ///
    /// Milestones use a uniform protocol across platforms: OSC marker followed
    /// by an alternating cursor-visibility fence (`CSI ?25l` / `CSI ?25h`).
    /// On Windows, `ConPTY` may deliver unrecognized OSC before rendered output;
    /// waiting for the expected fence guarantees prior rendered output has been
    /// consumed.
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

        let fence: &[u8] = {
            self.cursor_hidden = !self.cursor_hidden;
            if self.cursor_hidden { b"\x1b[?25l" } else { b"\x1b[?25h" }
        };

        let mut buf = [0u8; 4096];
        let mut milestone_match = 0usize;
        let mut found_milestone = false;
        let mut fence_match = 0usize;

        loop {
            let n = self.pty.read(&mut buf).expect("PTY read failed");
            assert!(n > 0, "EOF reached before milestone '{name}'");

            for byte in &buf[..n] {
                if !found_milestone {
                    if *byte == milestone[milestone_match] {
                        milestone_match += 1;
                        if milestone_match == milestone.len() {
                            found_milestone = true;
                            fence_match = 0;
                        }
                    } else {
                        milestone_match = usize::from(*byte == milestone[0]);
                    }
                    continue;
                }

                if *byte == fence[fence_match] {
                    fence_match += 1;
                    if fence_match == fence.len() {
                        return self.pty.screen_contents();
                    }
                } else {
                    fence_match = usize::from(*byte == fence[0]);
                }
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

    /// Returns the current terminal screen contents.
    ///
    /// # Panics
    ///
    /// Panics if the parser lock is poisoned.
    #[must_use]
    pub fn screen_contents(&self) -> String {
        self.pty.screen_contents()
    }
}

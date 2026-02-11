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
    /// Counts milestones to match the child's counter-based cursor column.
    milestone_counter: u16,
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
            reader: Reader { pty: pty_reader, child_handle, milestone_counter: 0 },
        })
    }
}

impl Reader {
    /// Reads from the PTY until a milestone with the given name is encountered.
    ///
    /// Returns the terminal screen contents at the moment the milestone is detected.
    ///
    /// On Windows, `ConPTY` delivers unrecognized OSC sequences via a fast
    /// pass-through path that may arrive before the rendered character output.
    /// Each milestone also moves the cursor to a unique position on the last
    /// row; since this cursor movement goes through `ConPTY`'s rendering
    /// pipeline, waiting for the cursor to reach the expected position
    /// guarantees all preceding character output has been consumed.
    ///
    /// # Panics
    ///
    /// Panics if the child process exits (EOF) before the named milestone is received,
    /// or if a read error occurs.
    #[must_use]
    pub fn expect_milestone(&mut self, name: &str) -> String {
        self.milestone_counter += 1;
        let expected_col = self.milestone_counter - 1; // 0-indexed

        let name_bytes = name.as_bytes();
        let mut buf = [0u8; 4096];
        let mut found = false;
        loop {
            for seq in self.pty.take_unhandled_osc_sequences() {
                if seq.first().is_some_and(|id| id == MILESTONE_OSC_ID)
                    && seq.get(1).is_some_and(|n| n == name_bytes)
                {
                    found = true;
                }
            }
            if found {
                let (_, col) = self.pty.cursor_position();
                if col == expected_col {
                    return self.pty.screen_contents();
                }
            }
            let n = self.pty.read(&mut buf).expect("PTY read failed");
            assert!(n > 0, "EOF reached before milestone '{name}'");
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

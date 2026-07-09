use std::io::{BufReader, Read};

pub use portable_pty::CommandBuilder;
use pty_terminal::terminal::{PtyReader, Terminal};
pub use pty_terminal::{
    ExitStatus,
    geo::ScreenSize,
    terminal::{ChildHandle, PtyWriter},
};

/// A test-oriented terminal that provides milestone-based synchronization.
///
/// Wraps a PTY terminal, splitting it into a [`PtyWriter`] for sending input
/// and a [`Reader`] that can wait for named milestones emitted by the child
/// process via [`pty_terminal_test_client::mark_milestone`].
pub struct TestTerminal {
    pub writer: PtyWriter,
    pub reader: Reader,
    pub child_handle: ChildHandle,
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
        let Terminal { pty_reader, pty_writer, child_handle, .. } = Terminal::spawn(size, cmd)?;
        Ok(Self {
            writer: pty_writer,
            reader: Reader { pty: BufReader::new(pty_reader), child_handle: child_handle.clone() },
            child_handle,
        })
    }
}

impl Reader {
    /// Returns the current terminal screen contents.
    #[must_use]
    pub fn screen_contents(&self) -> String {
        self.pty.get_ref().screen_contents()
    }

    /// Returns the screen contents with inline ANSI SGR escape codes preserved.
    /// Useful for snapshot tests that need to assert colour or style attributes.
    #[must_use]
    pub fn screen_contents_formatted(&self) -> Vec<u8> {
        self.pty.get_ref().screen_contents_formatted()
    }

    /// Reads from the PTY until a milestone with the given name is encountered.
    ///
    /// Returns the terminal screen contents at the moment the milestone is detected.
    ///
    /// Milestones use a uniform title token across platforms.
    ///
    /// # Panics
    ///
    /// Panics if the child process exits (EOF) before the named milestone is received,
    /// or if a read error occurs.
    #[must_use]
    pub fn expect_milestone(&mut self, name: &str) -> String {
        let mut buf = [0u8; 4096];

        loop {
            while let Some(title) = self.pty.get_ref().take_window_title() {
                if pty_terminal_test_client::decode_milestone_title(&title)
                    .is_some_and(|milestone| milestone == name)
                {
                    return self.screen_contents();
                }
            }

            let n = self.pty.read(&mut buf).expect("PTY read failed");
            assert!(n > 0, "EOF reached before milestone '{name}'");
        }
    }

    /// Reads all remaining PTY output until the child exits, then returns the exit status.
    ///
    /// # Errors
    ///
    /// Returns an error if waiting for the child process exit status fails.
    ///
    /// # Panics
    ///
    /// Panics if reading from the PTY fails.
    pub fn wait_for_exit(&mut self) -> anyhow::Result<ExitStatus> {
        let mut discard = Vec::new();
        self.pty.read_to_end(&mut discard).expect("PTY read_to_end failed");
        self.child_handle.wait()
    }
}

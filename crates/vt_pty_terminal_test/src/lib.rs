use std::io::{BufReader, Read as _};

pub use portable_pty::CommandBuilder;
use vt_pty_terminal::terminal::{PtyReader, Terminal};
pub use vt_pty_terminal::{
    ExitStatus,
    geo::ScreenSize,
    terminal::{ChildHandle, PtyWriter},
};

/// A test-oriented terminal that provides milestone-based synchronization.
///
/// Wraps a PTY terminal, splitting it into a [`PtyWriter`] for sending input
/// and a [`Reader`] that can wait for named milestones emitted by the child
/// process via [`vt_pty_terminal_test_client::mark_milestone`].
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
        // Producers on Windows wait for each milestone to be seen while
        // holding a lock shared by the processes in this terminal.
        #[cfg(windows)]
        let cmd = {
            use std::sync::atomic::{AtomicU64, Ordering};

            static NEXT_TERMINAL: AtomicU64 = AtomicU64::new(0);
            let mut cmd = cmd;
            let terminal = NEXT_TERMINAL.fetch_add(1, Ordering::Relaxed);
            cmd.env(
                vt_pty_terminal_test_client::MILESTONE_LOCK_ENV,
                format!("{}-{terminal}", std::process::id()),
            );
            cmd
        };
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
    /// Every milestone read before the named one is skipped.
    ///
    /// # Panics
    ///
    /// Panics if the child process exits (EOF) before the named milestone is received,
    /// or if a read error occurs.
    #[must_use]
    pub fn expect_milestone(&mut self, name: &str) -> String {
        let mut buf = [0u8; 4096];

        loop {
            if self.take_milestones().any(|milestone| milestone == name) {
                return self.screen_contents();
            }

            let n = self.pty.read(&mut buf).expect("PTY read failed");
            assert!(n > 0, "EOF reached before milestone '{name}'");
        }
    }

    /// Takes the names of the milestones read so far, letting each producer
    /// continue once its milestone is taken.
    fn take_milestones(&self) -> impl Iterator<Item = String> {
        std::iter::from_fn(|| {
            loop {
                let title = self.pty.get_ref().take_window_title()?;
                if let Some(milestone) = vt_pty_terminal_test_client::decode_milestone_title(&title)
                {
                    acknowledge(milestone.id);
                    return Some(milestone.name);
                }
            }
        })
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
        let mut buf = [0u8; 4096];
        loop {
            self.take_milestones().for_each(drop);
            if self.pty.read(&mut buf).expect("PTY read failed") == 0 {
                break;
            }
        }
        self.child_handle.wait()
    }
}

/// Signals the producer of milestone `id` that the milestone was seen.
///
/// See `vt_pty_terminal_test_client::mark_milestone` for why it waits.
#[cfg(windows)]
fn acknowledge(id: u128) {
    use winapi::um::{handleapi, synchapi, winnt};

    let name = vt_pty_terminal_test_client::milestone_ack_event_name(id)
        .encode_utf16()
        .chain([0])
        .collect::<Vec<_>>();
    // SAFETY: `name` is a valid NUL-terminated UTF-16 name.
    let event = unsafe { synchapi::OpenEventW(winnt::EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    // The producer doesn't wait if it didn't inherit the lock environment
    // variable, and closes the event once it is signaled.
    if event.is_null() {
        return;
    }
    // SAFETY: `event` is a valid event handle owned here.
    unsafe {
        synchapi::SetEvent(event);
        handleapi::CloseHandle(event);
    }
}

#[cfg(not(windows))]
const fn acknowledge(_id: u128) {}

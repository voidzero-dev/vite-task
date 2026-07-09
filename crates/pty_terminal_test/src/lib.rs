use std::{collections::VecDeque, io::Read};

pub use portable_pty::CommandBuilder;
use pty_terminal::terminal::{PtyReader, Terminal};
pub use pty_terminal::{
    ExitStatus,
    geo::ScreenSize,
    terminal::{ChildHandle, PtyWriter},
};

const MILESTONE_HYPERTEXT: char = '\u{200b}';

/// Tracks the two independently delivered parts of each milestone.
///
/// A milestone starts with an OSC 8 hyperlink carrying its name and contains a
/// zero-width printable character. On `ConPTY`, the OSC control sequence can be
/// forwarded before earlier screen updates, while the printable character
/// follows those updates through the asynchronous rendering path. A milestone
/// is therefore complete only after both parts have arrived.
///
/// Several OSC markers can overtake their anchors. The two queues preserve the
/// protocol order so an earlier marker's delayed anchor cannot complete a later
/// marker by mistake.
#[derive(Default)]
struct MilestoneTracker {
    /// Marker names whose rendered zero-width anchors have not arrived yet.
    awaiting_fence: VecDeque<String>,
    /// Marker names whose matching rendered anchors have arrived.
    completed: VecDeque<String>,
}

impl MilestoneTracker {
    fn take_completed(&mut self, name: &str) -> bool {
        // Keep unrelated completed milestones available for later calls. A PTY
        // read can contain more than the milestone currently being requested.
        self.completed
            .iter()
            .position(|completed| completed == name)
            .and_then(|index| self.completed.remove(index))
            .is_some()
    }
}

impl vte::Perform for MilestoneTracker {
    fn print(&mut self, character: char) {
        // `print` is called only for rendered characters, not for bytes inside
        // OSC metadata. ConPTY preserves the order of these rendered anchors,
        // so each anchor completes the oldest marker still awaiting one.
        if character == MILESTONE_HYPERTEXT
            && let Some(name) = self.awaiting_fence.pop_front()
        {
            self.completed.push_back(name);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // The decoder accepts only milestone hyperlink opens. Ordinary OSC
        // sequences and the empty OSC 8 close sequence are ignored.
        if let Some(name) = pty_terminal_test_client::decode_milestone_from_osc8_params(params) {
            self.awaiting_fence.push_back(name);
        }
    }
}

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
    /// Reads bytes and updates the terminal's primary `vt100` screen parser.
    ///
    /// This is deliberately not wrapped in `BufReader`: its read-ahead would
    /// let the primary parser consume bytes the milestone parser has not seen.
    pty: PtyReader,
    /// Observes the same byte stream to distinguish OSC markers from printable
    /// anchors. `vt100::Callbacks` exposes unhandled OSC sequences but has no
    /// callback for ordinary rendered characters, hence this small second parser.
    milestone_parser: vte::Parser,
    /// Persists protocol state across reads and `expect_milestone` calls.
    milestone_tracker: MilestoneTracker,
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
            reader: Reader {
                pty: pty_reader,
                milestone_parser: vte::Parser::new(),
                milestone_tracker: MilestoneTracker::default(),
                child_handle: child_handle.clone(),
            },
            child_handle,
        })
    }
}

impl Reader {
    /// Reads once while keeping the screen parser and milestone parser in lockstep.
    ///
    /// All PTY draining, including shutdown, must go through this method. Reading
    /// directly from `pty` would update the screen while silently skipping those
    /// bytes in the milestone protocol state.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.pty.read(buf)?;
        self.milestone_parser.advance(&mut self.milestone_tracker, &buf[..n]);

        // `PtyReader`'s primary parser also records the OSC sequences. The
        // dedicated tracker above owns milestone handling, so discard this
        // duplicate copy rather than letting it grow for the lifetime of a test.
        drop(self.pty.take_unhandled_osc_sequences());
        Ok(n)
    }

    /// Returns terminal screen contents with milestone hyperlink text removed.
    #[must_use]
    pub fn screen_contents(&self) -> String {
        let mut contents = self.pty.screen_contents();
        contents.retain(|ch| ch != MILESTONE_HYPERTEXT);
        contents
    }

    /// Returns the screen contents with inline ANSI SGR escape codes preserved.
    /// Useful for snapshot tests that need to assert colour or style attributes.
    #[must_use]
    pub fn screen_contents_formatted(&self) -> Vec<u8> {
        self.pty.screen_contents_formatted()
    }

    /// Reads from the PTY until a milestone with the given name is encountered.
    ///
    /// Returns the terminal screen contents at the moment the milestone is detected.
    ///
    /// Milestones use a uniform protocol across platforms: the milestone name
    /// is encoded in an OSC 8 hyperlink URI. A zero-width hyperlink anchor follows
    /// each marker through the rendered output path. The reader waits for both the
    /// marker and its corresponding anchor before returning, then strips the anchor
    /// from the returned screen contents. Marker and anchor parsing is incremental,
    /// so either sequence may be split across PTY reads or share a read with other
    /// milestones.
    ///
    /// # Panics
    ///
    /// Panics if the child process exits (EOF) before the named milestone is received,
    /// or if a read error occurs.
    #[must_use]
    pub fn expect_milestone(&mut self, name: &str) -> String {
        let mut buf = [0u8; 4096];

        loop {
            if self.milestone_tracker.take_completed(name) {
                return self.screen_contents();
            }

            let n = self.read(&mut buf).expect("PTY read failed");
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
        let mut buf = [0u8; 4096];
        while self.read(&mut buf).expect("PTY read failed") > 0 {}
        self.child_handle.wait()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_without_fence(name: &str) -> Vec<u8> {
        // Model ConPTY's fast control path by delivering the complete OSC marker
        // before its printable anchor reaches the output pipe.
        let mut marker = pty_terminal_test_client::encoded_milestone(name);
        let index = marker
            .windows(pty_terminal_test_client::MILESTONE_RENDER_FENCE.len())
            .position(|window| window == pty_terminal_test_client::MILESTONE_RENDER_FENCE)
            .unwrap();
        marker.drain(index..index + pty_terminal_test_client::MILESTONE_RENDER_FENCE.len());
        marker
    }

    fn advance(parser: &mut vte::Parser, tracker: &mut MilestoneTracker, bytes: &[u8]) {
        parser.advance(tracker, bytes);
    }

    #[test]
    fn milestone_waits_for_rendered_fence() {
        let mut parser = vte::Parser::new();
        let mut tracker = MilestoneTracker::default();

        // Receiving the marker and subsequent printable output is insufficient:
        // only the protocol's rendered anchor establishes the screen barrier.
        advance(&mut parser, &mut tracker, &marker_without_fence("target"));
        advance(&mut parser, &mut tracker, b"rendered output");
        assert!(!tracker.take_completed("target"));

        advance(&mut parser, &mut tracker, pty_terminal_test_client::MILESTONE_RENDER_FENCE);
        assert!(tracker.take_completed("target"));
    }

    #[test]
    fn milestone_parses_across_every_chunk_boundary() {
        let marker = pty_terminal_test_client::encoded_milestone("target");

        for split in 0..=marker.len() {
            let mut parser = vte::Parser::new();
            let mut tracker = MilestoneTracker::default();
            advance(&mut parser, &mut tracker, &marker[..split]);
            advance(&mut parser, &mut tracker, &marker[split..]);
            assert!(tracker.take_completed("target"), "failed at split {split}");
        }
    }

    #[test]
    fn rendered_fences_complete_overtaken_markers_in_order() {
        let mut parser = vte::Parser::new();
        let mut tracker = MilestoneTracker::default();
        let mut markers = marker_without_fence("first");
        markers.extend(marker_without_fence("second"));

        // Both controls overtake rendering. The first anchor must still complete
        // `first`, never whichever marker the caller happens to be waiting for.
        advance(&mut parser, &mut tracker, &markers);
        advance(&mut parser, &mut tracker, pty_terminal_test_client::MILESTONE_RENDER_FENCE);
        assert!(tracker.take_completed("first"));
        assert!(!tracker.take_completed("second"));

        advance(&mut parser, &mut tracker, pty_terminal_test_client::MILESTONE_RENDER_FENCE);
        assert!(tracker.take_completed("second"));
    }
}

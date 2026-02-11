/// The OSC parameter ID used for the milestone protocol.
///
/// Milestone OSC format: `ESC ] 9999 ; <name> BEL`
///
/// The vt100 parser splits by `;`, so `unhandled_osc` receives
/// `params = [b"9999", b"<name>"]`.
pub const MILESTONE_OSC_ID: &[u8] = b"9999";
/// A non-visual UTF-8 fence appended after each milestone OSC.
pub const MILESTONE_FENCE: &[u8] = "\u{200b}".as_bytes();

/// Emits a milestone marker as a private OSC escape sequence.
///
/// The child process calls this to signal it has reached a named synchronization
/// point. The test harness (via `pty_terminal_test::Reader::expect_milestone`)
/// detects this marker and returns the screen contents at that point.
///
/// On Windows, `ConPTY` passes unrecognized OSC sequences directly to the
/// output pipe (synchronous, inline with input processing), while rendered
/// character output is generated asynchronously by a separate output thread
/// that polls the console buffer. This means the OSC can arrive at the
/// reader before preceding character output has been emitted.
///
/// Each milestone also appends a non-visual zero-width-space fence (`U+200B`,
/// UTF-8 `E2 80 8B`). This keeps a uniform protocol across platforms. On
/// Windows, waiting for this rendered character after the OSC confirms prior
/// rendered output has been consumed.
///
/// When the `testing` feature is disabled, this is a no-op.
///
/// # Panics
///
/// Panics if writing to stdout fails.
#[cfg(feature = "testing")]
pub fn mark_milestone(name: &str) {
    use std::io::{Write, stdout};

    let mut stdout = stdout();
    write!(stdout, "\x1b]9999;{name}\x07").unwrap();
    stdout.write_all(MILESTONE_FENCE).unwrap();

    stdout.flush().unwrap();
}

/// Emits a milestone marker as a private OSC escape sequence.
///
/// When the `testing` feature is disabled, this is a no-op.
#[cfg(not(feature = "testing"))]
pub const fn mark_milestone(_name: &str) {}

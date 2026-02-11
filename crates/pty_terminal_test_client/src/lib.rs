/// The OSC parameter ID used for the milestone protocol.
///
/// Milestone OSC format: `ESC ] 9999 ; <name> BEL`
///
/// The vt100 parser splits by `;`, so `unhandled_osc` receives
/// `params = [b"9999", b"<name>"]`.
pub const MILESTONE_OSC_ID: &[u8] = b"9999";

/// Emits a milestone marker as a private OSC escape sequence.
///
/// The child process calls this to signal it has reached a named synchronization
/// point. The test harness (via `pty_terminal_test::Reader::expect_milestone`)
/// detects this marker and returns the screen contents at that point.
///
/// On Windows, `ConPTY` delivers unrecognized OSC sequences via a fast
/// pass-through path that may arrive before preceding rendered character output.
/// To allow the reader to detect when rendering has caught up, each milestone
/// also moves the cursor to a unique position on the last row (column = call
/// count). The cursor movement goes through `ConPTY`'s rendering pipeline,
/// guaranteeing it arrives after all preceding character output.
///
/// When the `testing` feature is disabled, this is a no-op.
///
/// # Panics
///
/// Panics if writing to stdout fails.
#[cfg(feature = "testing")]
pub fn mark_milestone(name: &str) {
    use std::{
        io::{Write, stdout},
        sync::atomic::{AtomicU16, Ordering},
    };

    static COUNTER: AtomicU16 = AtomicU16::new(0);
    let col = COUNTER.fetch_add(1, Ordering::Relaxed) + 1; // 1-based for VT

    let mut stdout = stdout();
    // OSC milestone (pass-through on ConPTY) + cursor move to last row (rendering pipeline)
    write!(stdout, "\x1b]9999;{name}\x07\x1b[999;{col}H").unwrap();
    stdout.flush().unwrap();
}

/// Emits a milestone marker as a private OSC escape sequence.
///
/// When the `testing` feature is disabled, this is a no-op.
#[cfg(not(feature = "testing"))]
pub const fn mark_milestone(_name: &str) {}

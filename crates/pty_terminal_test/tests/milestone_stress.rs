/// Stress test for milestone PTY tests to reproduce flaky SIGSEGV on musl.
///
/// The original `milestone` tests occasionally crash with SIGSEGV on Alpine/musl
/// (see <https://github.com/voidzero-dev/vite-task/actions/runs/23328556726/job/67854932784>).
/// This stress test runs the same PTY operations repeatedly and concurrently to
/// amplify whatever race condition or memory issue triggers the crash.
use std::io::Write;

use ntest::timeout;
use portable_pty::CommandBuilder;
use pty_terminal::geo::ScreenSize;
use pty_terminal_test::TestTerminal;
use subprocess_test::command_for_fn;

fn run_milestone_raw_mode_keystrokes() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Read, Write, stdout};

        crossterm::terminal::enable_raw_mode().unwrap();
        pty_terminal_test_client::mark_milestone("ready");

        let mut stdin = std::io::stdin();
        let mut stdout = stdout();
        let mut byte = [0u8; 1];

        loop {
            stdin.read_exact(&mut byte).unwrap();
            let ch = byte[0] as char;
            write!(stdout, "\x1b[2J\x1b[H{ch}").unwrap();
            stdout.flush().unwrap();
            pty_terminal_test_client::mark_milestone("keystroke");
            if ch == 'q' {
                break;
            }
        }

        crossterm::terminal::disable_raw_mode().unwrap();
    }));

    let TestTerminal { mut writer, mut reader, child_handle: _ } =
        TestTerminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    let _ = reader.expect_milestone("ready");

    writer.write_all(b"a").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "a");

    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    let status = reader.wait_for_exit().unwrap();
    assert!(status.success());
}

fn run_milestone_does_not_pollute_screen() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Read, Write, stdout};

        crossterm::terminal::enable_raw_mode().unwrap();
        pty_terminal_test_client::mark_milestone("ready");

        let mut stdin = std::io::stdin();
        let mut stdout = stdout();
        let mut byte = [0u8; 1];

        loop {
            stdin.read_exact(&mut byte).unwrap();
            let ch = byte[0] as char;
            write!(stdout, "{ch}").unwrap();
            stdout.flush().unwrap();
            pty_terminal_test_client::mark_milestone("keystroke");
            if ch == 'q' {
                break;
            }
        }

        crossterm::terminal::disable_raw_mode().unwrap();
    }));

    let TestTerminal { mut writer, mut reader, child_handle: _ } =
        TestTerminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    let _ = reader.expect_milestone("ready");

    writer.write_all(b"a").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "a");

    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    let status = reader.wait_for_exit().unwrap();
    assert!(status.success());
}

#[test]
#[timeout(60_000)]
fn milestone_stress_sequential() {
    for _ in 0..20 {
        run_milestone_raw_mode_keystrokes();
        run_milestone_does_not_pollute_screen();
    }
}

#[test]
#[timeout(60_000)]
fn milestone_stress_concurrent() {
    // Run multiple iterations where both milestone tests execute concurrently
    // via threads, mimicking the parallel test execution in `cargo test`.
    for _ in 0..20 {
        std::thread::scope(|s| {
            s.spawn(run_milestone_raw_mode_keystrokes);
            s.spawn(run_milestone_does_not_pollute_screen);
        });
    }
}

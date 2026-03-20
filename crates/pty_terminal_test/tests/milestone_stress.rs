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

/// Install a signal handler that prints debug info on SIGSEGV.
#[cfg(unix)]
#[ctor::ctor]
unsafe fn install_sigsegv_handler() {
    unsafe extern "C" fn handler(sig: libc::c_int) {
        unsafe {
            let msg = b"SIGSEGV caught! Signal: ";
            libc::write(2, msg.as_ptr().cast(), msg.len());
            let digit = b'0' + (sig as u8);
            libc::write(2, (&digit) as *const u8 as _, 1);
            let nl = b"\n/proc/self/maps:\n";
            libc::write(2, nl.as_ptr().cast(), nl.len());

            let fd = libc::open(b"/proc/self/maps\0".as_ptr().cast(), libc::O_RDONLY);
            if fd >= 0 {
                let mut buf = [0u8; 4096];
                loop {
                    let n = libc::read(fd, buf.as_mut_ptr().cast(), buf.len());
                    if n <= 0 {
                        break;
                    }
                    libc::write(2, buf.as_ptr().cast(), n as usize);
                }
                libc::close(fd);
            }

            libc::signal(libc::SIGSEGV, libc::SIG_DFL);
            libc::raise(libc::SIGSEGV);
        }
    }

    unsafe {
        // Set up alternate signal stack for handling stack overflows
        let stack_size = 64 * 1024;
        let stack = libc::mmap(
            std::ptr::null_mut(),
            stack_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );
        if stack != libc::MAP_FAILED {
            let ss = libc::stack_t { ss_sp: stack, ss_flags: 0, ss_size: stack_size };
            libc::sigaltstack(&ss, std::ptr::null_mut());
        }

        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = handler as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
    }
}

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
#[timeout(120_000)]
fn milestone_stress_sequential() {
    for _ in 0..100 {
        run_milestone_raw_mode_keystrokes();
        run_milestone_does_not_pollute_screen();
    }
}

#[test]
#[timeout(120_000)]
fn milestone_stress_concurrent() {
    // Run multiple iterations where both milestone tests execute concurrently
    // via threads, mimicking the parallel test execution in `cargo test`.
    for _ in 0..100 {
        std::thread::scope(|s| {
            s.spawn(run_milestone_raw_mode_keystrokes);
            s.spawn(run_milestone_does_not_pollute_screen);
        });
    }
}

#[test]
#[timeout(120_000)]
fn milestone_stress_high_concurrency() {
    // Run many PTY sessions in parallel to stress thread/PTY resource handling.
    for _ in 0..20 {
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(run_milestone_raw_mode_keystrokes);
                s.spawn(run_milestone_does_not_pollute_screen);
            }
        });
    }
}

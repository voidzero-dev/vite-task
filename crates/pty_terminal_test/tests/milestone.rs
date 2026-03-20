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
    fn write_hex(fd: libc::c_int, val: usize) {
        let mut hex_buf = [0u8; 18];
        hex_buf[0] = b'0';
        hex_buf[1] = b'x';
        let mut v = val;
        for i in (2..18).rev() {
            let nibble = (v & 0xf) as u8;
            hex_buf[i] = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            v >>= 4;
        }
        unsafe { libc::write(fd, hex_buf.as_ptr().cast(), 18) };
    }
    fn write_str(fd: libc::c_int, s: &[u8]) {
        unsafe { libc::write(fd, s.as_ptr().cast(), s.len()) };
    }

    unsafe extern "C" fn handler(
        _sig: libc::c_int,
        info: *mut libc::siginfo_t,
        context: *mut libc::c_void,
    ) {
        unsafe {
            write_str(2, b"\n=== SIGSEGV DEBUG INFO ===\n");

            // Fault address from siginfo
            write_str(2, b"Fault address (si_addr): ");
            if !info.is_null() {
                write_hex(2, (*info).si_addr() as usize);
            } else {
                write_str(2, b"(null siginfo)");
            }

            // Crashing thread's RSP from ucontext
            write_str(2, b"\nCrashing RSP: ");
            #[cfg(target_arch = "x86_64")]
            if !context.is_null() {
                let uc = context as *const libc::ucontext_t;
                let rsp = (*uc).uc_mcontext.gregs[libc::REG_RSP as usize] as usize;
                write_hex(2, rsp);
                write_str(2, b"\nCrashing RIP: ");
                let rip = (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as usize;
                write_hex(2, rip);
            }

            // Handler's own RSP (on sigaltstack if configured)
            write_str(2, b"\nHandler RSP:  ");
            let handler_sp: usize;
            #[cfg(target_arch = "x86_64")]
            {
                core::arch::asm!("mov {}, rsp", out(reg) handler_sp);
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                handler_sp = 0;
            }
            write_hex(2, handler_sp);

            write_str(2, b"\n/proc/self/maps:\n");
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
            write_str(2, b"=== END SIGSEGV DEBUG INFO ===\n");

            libc::signal(libc::SIGSEGV, libc::SIG_DFL);
            libc::raise(libc::SIGSEGV);
        }
    }

    unsafe {
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

#[test]
#[timeout(5000)]
fn milestone_raw_mode_keystrokes() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Read, Write, stdout};

        // Enable raw mode (cross-platform via crossterm)
        crossterm::terminal::enable_raw_mode().unwrap();

        // Signal that raw mode is ready
        pty_terminal_test_client::mark_milestone("ready");

        let mut stdin = std::io::stdin();
        let mut stdout = stdout();
        let mut byte = [0u8; 1];

        loop {
            stdin.read_exact(&mut byte).unwrap();
            let ch = byte[0] as char;

            // Clear screen and print the keystroke at top-left
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

    // Wait for the subprocess to be ready
    let _ = reader.expect_milestone("ready");

    // Write 'a', expect keystroke, verify screen
    writer.write_all(b"a").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "a");

    // Write 'b', expect keystroke, verify screen
    writer.write_all(b"b").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "b");

    // Write 'c', expect keystroke, verify screen
    writer.write_all(b"c").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "c");

    // Write 'q' to quit and wait for the child to exit
    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    let status = reader.wait_for_exit().unwrap();
    assert!(status.success());
}

/// Verifies that the non-visual milestone fence in `mark_milestone` does not
/// pollute `screen_contents()`. The subprocess appends characters without
/// clearing the screen, so any leftover space would appear between them.
#[test]
#[timeout(5000)]
fn milestone_does_not_pollute_screen() {
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

            // Append the character without clearing the screen
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

    writer.write_all(b"b").unwrap();
    writer.flush().unwrap();
    let screen = reader.expect_milestone("keystroke");
    assert_eq!(screen.trim(), "ab");

    writer.write_all(b"q").unwrap();
    writer.flush().unwrap();
    let status = reader.wait_for_exit().unwrap();
    assert!(status.success());
}

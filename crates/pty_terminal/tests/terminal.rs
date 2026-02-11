use std::io::{IsTerminal, Write, stderr, stdin, stdout};

use ntest::timeout;
use portable_pty::CommandBuilder;
use pty_terminal::{geo::ScreenSize, terminal::Terminal};
use subprocess_test::command_for_fn;

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn is_terminal() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        println!("{} {} {}", stdin().is_terminal(), stdout().is_terminal(), stderr().is_terminal());
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert_eq!(output.trim(), "true true true");
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_single() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        println!("hello world");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("hello").unwrap();
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // After reading until "hello", the buffer should contain " world"
    // read_to_end should process the buffered data and continue reading
    assert!(output.contains("world"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_multiple_sequential() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("first second third");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("first").unwrap();
    terminal.read_until("second").unwrap();
    terminal.read_until("third").unwrap();
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // All three words should be in the screen
    assert!(output.contains("first"));
    assert!(output.contains("second"));
    assert!(output.contains("third"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_not_found() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("hello world");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let result = terminal.read_until("nonexistent");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Expected string not found"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_with_read_to_end() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("prefix middle suffix");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    terminal.read_until("middle").unwrap();
    // At this point, " suffix" should be buffered
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // The full output should include everything
    assert!(output.contains("prefix"));
    assert!(output.contains("middle"));
    assert!(output.contains("suffix"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_boundary_spanning() {
    // Test that read_until works when the expected string may span across read() boundaries.
    // Boundary spanning is about the reader side: the PTY reader may return partial data even
    // from a single write. We print the full string at once because on Windows, ConPTY
    // reprocesses output and can insert escape sequences between individually-printed characters.
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("abcdef");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    // Search for a pattern that's likely to span boundaries
    terminal.read_until("abcd").unwrap();
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("abcdef"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_exact_boundary() {
    // Test where we search for something at the exact boundary
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("firstsecond");
        let _ = stdout().flush();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    // This should find "second" even if "first" was in a previous read
    terminal.read_until("second").unwrap();
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("first"));
    assert!(output.contains("second"));
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_until_after_read_to_end() {
    // Test that read_until works with data that comes after EOF
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        println!("hello world foo bar");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Use read_until first to consume part of the data
    terminal.read_until("world").unwrap();

    // Read everything else
    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert!(output.contains("hello world foo bar"));

    // After read_to_end, buffer is empty and we're at EOF
    // Trying to find anything should fail
    let result = terminal.read_until("bar");
    assert!(result.is_err());
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn write_basic_echo() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{BufRead, Write, stdin, stdout};
        let stdin = stdin();
        let mut stdout = stdout();
        let first_line = stdin.lock().lines().map_while(Result::ok).next();
        if let Some(line) = first_line {
            print!("{line}");
            stdout.flush().unwrap();
        }
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Write data to the terminal
    terminal.write(b"hello world\n").unwrap();

    // Read until we see the echo
    terminal.read_until("hello world").unwrap();
    let _ = terminal.read_to_end().unwrap();

    let output = terminal.screen_contents();
    // PTY echoes the input, so we see "hello world\nhello world"
    assert_eq!(output.trim(), "hello world\nhello world");
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn write_multiple_lines() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{BufRead, Write, stdin, stdout};
        let stdin = stdin();
        let mut stdout = stdout();
        for line in stdin.lock().lines().map_while(Result::ok) {
            print!("Echo: {line}");
            stdout.flush().unwrap();
            if line == "third" {
                break;
            }
        }
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    terminal.write(b"first\n").unwrap();
    terminal.read_until("Echo: first").unwrap();

    terminal.write(b"second\n").unwrap();
    terminal.read_until("Echo: second").unwrap();

    terminal.write(b"third\n").unwrap();
    terminal.read_until("Echo: third").unwrap();

    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    // PTY echoes input, so we see both the typed input and the echo response
    assert_eq!(output.trim(), "first\nEcho: firstsecond\nEcho: secondthird\nEcho: third");
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn write_after_exit() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        print!("exiting");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Read all output - this blocks until child exits and EOF is reached
    let _ = terminal.read_to_end().unwrap();

    // The background thread should have set writer to None by now
    // since read_to_end only returns after EOF (child exit)
    // Writing should fail with either our custom error or an I/O error
    let result = terminal.write(b"too late\n");
    assert!(result.is_err());
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn write_interactive_prompt() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Write, stdin, stdout};
        let mut stdout = stdout();
        print!("Name: ");
        stdout.flush().unwrap();

        let mut input = std::string::String::new();
        stdin().read_line(&mut input).unwrap();
        print!("Hello, {}", input.trim());
        stdout.flush().unwrap();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Wait for prompt
    terminal.read_until("Name:").unwrap();

    // Send response
    terminal.write(b"Alice\n").unwrap();

    // Wait for greeting
    terminal.read_until("Hello, Alice").unwrap();

    let _ = terminal.read_to_end().unwrap();
    let output = terminal.screen_contents();
    assert_eq!(output.trim(), "Name: Alice\nHello, Alice");
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn resize_terminal() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Write, stdin, stdout};
        #[cfg(unix)]
        use std::sync::Arc;
        #[cfg(unix)]
        use std::sync::atomic::{AtomicBool, Ordering};

        // Cross-platform function to get terminal size
        fn get_size() -> (u16, u16) {
            if let Some((terminal_size::Width(w), terminal_size::Height(h))) =
                terminal_size::terminal_size()
            {
                (h, w)
            } else {
                (0, 0)
            }
        }

        #[cfg(unix)]
        let resized = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        let resized_clone = Arc::clone(&resized);

        // Install SIGWINCH handler on Unix
        #[cfg(unix)]
        // SAFETY: The closure only performs an atomic store, which is signal-safe.
        unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGWINCH, move || {
                resized_clone.store(true, Ordering::SeqCst);
            })
            .unwrap();
        }

        // Print initial size
        let (rows, cols) = get_size();
        println!("initial: {rows} {cols}");
        stdout().flush().unwrap();

        // Wait for input to synchronize
        let mut input = std::string::String::new();
        stdin().read_line(&mut input).unwrap();

        // On Unix, check if resize signal was detected
        #[cfg(unix)]
        {
            if resized.load(Ordering::SeqCst) {
                println!("RESIZE_DETECTED");
            }
        }

        // On Windows, ConPTY resizes synchronously - detect by checking size change
        #[cfg(windows)]
        {
            let (new_rows, new_cols) = get_size();
            if (new_rows, new_cols) != (rows, cols) {
                println!("RESIZE_DETECTED");
            }
        }

        // Print new size
        let (rows, cols) = get_size();
        println!("resized: {rows} {cols}");
        stdout().flush().unwrap();
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Read initial size
    terminal.read_until("initial: 80 80").unwrap();

    // Perform resize
    terminal.resize(ScreenSize { rows: 40, cols: 40 }).unwrap();

    // Signal the process to continue and check resize
    terminal.write(b"\n").unwrap();

    // Verify resize was detected (SIGWINCH on Unix, synchronous on Windows)
    terminal.read_until("RESIZE_DETECTED").unwrap();

    // Verify new size is correct
    terminal.read_until("resized: 40 40").unwrap();

    let _ = terminal.read_to_end().unwrap();
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn send_ctrl_c_interrupts_process() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        use std::io::{Write, stdout};

        // On Windows, clear the "ignore CTRL_C" flag set by Rust runtime
        // so that CTRL_C_EVENT reaches the ctrlc handler.
        #[cfg(windows)]
        {
            // SAFETY: Declaring correct signature for SetConsoleCtrlHandler from kernel32.
            unsafe extern "system" {
                fn SetConsoleCtrlHandler(
                    handler: Option<unsafe extern "system" fn(u32) -> i32>,
                    add: i32,
                ) -> i32;
            }

            // SAFETY: Clearing the "ignore CTRL_C" flag so handlers are invoked.
            unsafe {
                SetConsoleCtrlHandler(None, 0); // FALSE = remove ignore
            }
        }

        ctrlc::set_handler(move || {
            // Write directly and exit from the handler to avoid races.
            use std::io::Write;
            let _ = write!(std::io::stdout(), "INTERRUPTED");
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        })
        .unwrap();

        println!("ready");
        stdout().flush().unwrap();

        // Block until Ctrl+C handler exits the process.
        loop {
            std::thread::park();
        }
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();

    // Wait for process to be ready
    terminal.read_until("ready").unwrap();

    // Send Ctrl+C
    terminal.send_ctrl_c().unwrap();

    // Verify interruption was detected
    terminal.read_until("INTERRUPTED").unwrap();

    let _ = terminal.read_to_end().unwrap();
}

#[test]
#[timeout(5000)]
#[expect(clippy::print_stdout, reason = "subprocess test output")]
fn read_to_end_returns_exit_status_success() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        println!("success");
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let status = terminal.read_to_end().unwrap();
    assert!(status.success());
    assert_eq!(status.exit_code(), 0);
}

#[test]
#[timeout(5000)]
fn read_to_end_returns_exit_status_nonzero() {
    let cmd = CommandBuilder::from(command_for_fn!((), |(): ()| {
        std::process::exit(42);
    }));

    let mut terminal = Terminal::spawn(ScreenSize { rows: 80, cols: 80 }, cmd).unwrap();
    let status = terminal.read_to_end().unwrap();
    assert!(!status.success());
    assert_eq!(status.exit_code(), 42);
}

/// exit-on-ctrlc
///
/// Sets up a Ctrl+C handler, emits a "ready" milestone, then waits.
/// When Ctrl+C is received, prints "ctrl-c received" and exits.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // On Windows, an ancestor process (e.g. cargo, the test harness) may have
    // been created with CREATE_NEW_PROCESS_GROUP, which implicitly calls
    // SetConsoleCtrlHandler(NULL, TRUE) and sets CONSOLE_IGNORE_CTRL_C in the
    // PEB's ConsoleFlags. This flag is inherited by all descendants and takes
    // precedence over registered handlers — CTRL_C_EVENT is silently dropped.
    // Clear it so our handler can fire.
    // Ref: https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
    #[cfg(windows)]
    {
        // SAFETY: Passing (None, FALSE) clears the per-process CTRL_C ignore flag.
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        // SAFETY: Clearing the inherited ignore flag.
        unsafe {
            SetConsoleCtrlHandler(None, 0);
        }
    }

    ctrlc::set_handler(move || {
        use std::io::Write;
        let _ = write!(std::io::stdout(), "ctrl-c received");
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    })?;

    pty_terminal_test_client::mark_milestone("ready");
    // Print a newline so the milestone bytes get flushed through line-buffered
    // writers (labeled/grouped log modes).
    println!();

    loop {
        std::thread::park();
    }
}

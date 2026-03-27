// This is a standalone test utility binary that deliberately uses std types
// rather than the project's custom types (vite_str, vite_path, etc.).
#![expect(clippy::disallowed_types, reason = "standalone test utility uses std types")]
#![expect(clippy::disallowed_macros, reason = "standalone test utility uses std macros")]
#![expect(clippy::disallowed_methods, reason = "standalone test utility uses std methods")]
#![expect(clippy::print_stderr, reason = "CLI tool error output")]
#![expect(clippy::print_stdout, reason = "CLI tool output")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vtt <subcommand> [args...]");
        eprintln!(
            "Subcommands: barrier, check-tty, print, print-cwd, print-env, print-file, read-stdin, replace-file-content, touch-file"
        );
        std::process::exit(1);
    }

    let result: Result<(), Box<dyn std::error::Error>> = match args[1].as_str() {
        "barrier" => cmd_barrier(&args[2..]),
        "check-tty" => {
            cmd_check_tty();
            Ok(())
        }
        "print" => {
            cmd_print(&args[2..]);
            Ok(())
        }
        "print-cwd" => cmd_print_cwd(),
        "print-env" => cmd_print_env(&args[2..]),
        "print-file" => cmd_print_file(&args[2..]),
        "read-stdin" => cmd_read_stdin(),
        "replace-file-content" => cmd_replace_file_content(&args[2..]),
        "touch-file" => cmd_touch_file(&args[2..]),
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

/// barrier `<dir>` `<prefix>` `<count>` \[--exit=`<code>`\] \[--hang\] \[--daemonize\]
///
/// Cross-platform concurrency barrier for testing.
/// Creates `<dir>/<prefix>_<pid>`, then polls until `<count>` files matching
/// `<prefix>_*` exist in `<dir>`.
///
/// Options:
/// - `--exit=<code>`: Exit with the given code after the barrier is met.
/// - `--hang`: Keep process alive after the barrier (for kill tests).
/// - `--daemonize`: Close stdout/stderr but keep process alive (for daemon kill tests).
fn cmd_barrier(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut positional: Vec<&str> = Vec::new();
    let mut exit_code: i32 = 0;
    let mut hang = false;
    let mut daemonize = false;

    for arg in args {
        if let Some(code) = arg.strip_prefix("--exit=") {
            exit_code = code.parse()?;
        } else if arg == "--hang" {
            hang = true;
        } else if arg == "--daemonize" {
            daemonize = true;
        } else {
            positional.push(arg.as_str());
        }
    }

    if positional.len() < 3 {
        return Err(
            "Usage: vtt barrier <dir> <prefix> <count> [--exit=<code>] [--hang] [--daemonize]"
                .into(),
        );
    }

    let dir = std::path::Path::new(positional[0]);
    let prefix = positional[1];
    let count: usize = positional[2].parse()?;

    std::fs::create_dir_all(dir)?;

    // Create this participant's marker file.
    let pid = std::process::id();
    let marker = dir.join(std::format!("{prefix}_{pid}"));
    std::fs::write(&marker, "")?;

    // Wait until <count> matching files exist using filesystem notifications.
    let prefix_match = std::format!("{prefix}_");
    let count_matches = |d: &std::path::Path| -> Result<bool, Box<dyn std::error::Error>> {
        Ok(std::fs::read_dir(d)?
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(prefix_match.as_str()))
            .count()
            >= count)
    };
    if !count_matches(dir)? {
        use notify::Watcher as _;
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
        // Re-check after setting up the watcher to avoid missing events created
        // between our marker write and the watcher registration.
        if !count_matches(dir)? {
            for _ in rx {
                if count_matches(dir)? {
                    break;
                }
            }
        }
    }

    if daemonize {
        // Close stdout/stderr but keep the process alive. Simulates a daemon that
        // detaches from stdio — tests that the runner can still kill such processes.
        // Closing the fds gives the parent's pipe an EOF.
        // SAFETY: fds 1 and 2 are always valid (stdout/stderr).
        unsafe {
            libc::close(1);
            libc::close(2);
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    if hang {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    std::process::exit(exit_code);
}

fn cmd_check_tty() {
    use std::io::IsTerminal as _;
    let stdin_tty = if std::io::stdin().is_terminal() { "tty" } else { "not-tty" };
    let stdout_tty = if std::io::stdout().is_terminal() { "tty" } else { "not-tty" };
    let stderr_tty = if std::io::stderr().is_terminal() { "tty" } else { "not-tty" };
    println!("stdin:{stdin_tty}");
    println!("stdout:{stdout_tty}");
    println!("stderr:{stderr_tty}");
}

fn cmd_print(args: &[String]) {
    println!("{}", args.join(" "));
}

fn cmd_print_cwd() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    println!("{}", cwd.display());
    Ok(())
}

fn cmd_print_env(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("Usage: vtt print-env <VAR_NAME>".into());
    }
    let value = std::env::var(&args[0]).unwrap_or_else(|_| "(undefined)".to_string());
    println!("{value}");
    Ok(())
}

fn cmd_print_file(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for file in args {
        match std::fs::read(file) {
            Ok(content) => out.write_all(&content)?,
            Err(_) => eprintln!("{file}: not found"),
        }
    }
    Ok(())
}

fn cmd_read_stdin() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read as _, Write as _};
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut buf = [0u8; 8192];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => stdout.write_all(&buf[..n])?,
        }
    }
    Ok(())
}

fn cmd_replace_file_content(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 3 {
        return Err("Usage: vtt replace-file-content <filename> <searchValue> <newValue>".into());
    }
    let filename = &args[0];
    let search_value = &args[1];
    let new_value = &args[2];

    let filepath = std::path::Path::new(filename).canonicalize()?;
    let content = std::fs::read_to_string(&filepath)?;
    if !content.contains(search_value) {
        return Err(std::format!("searchValue not found in {filename}: {search_value:?}").into());
    }
    let new_content = content.replacen(search_value, new_value, 1);
    std::fs::write(&filepath, new_content)?;
    Ok(())
}

fn cmd_touch_file(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("Usage: vtt touch-file <filename>".into());
    }
    let _file = std::fs::OpenOptions::new().read(true).write(true).open(&args[0])?;
    Ok(())
}

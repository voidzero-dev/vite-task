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
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use notify::Watcher as _;
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
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
    if !count_matches(dir)? {
        for _ in rx {
            if count_matches(dir)? {
                break;
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
            std::thread::park();
        }
    }

    if hang {
        loop {
            std::thread::park();
        }
    }

    std::process::exit(exit_code);
}

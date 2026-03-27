// This is a standalone test utility binary that deliberately uses std types
// rather than the project's custom types (vite_str, vite_path, etc.).
#![expect(clippy::disallowed_types, reason = "standalone test utility uses std types")]
#![expect(clippy::disallowed_macros, reason = "standalone test utility uses std macros")]
#![expect(clippy::disallowed_methods, reason = "standalone test utility uses std methods")]
#![expect(clippy::print_stderr, reason = "CLI tool error output")]
#![expect(clippy::print_stdout, reason = "CLI tool output")]

use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Cross-platform concurrency barrier for testing.
    /// Creates `<dir>/<prefix>_<pid>`, then polls until `<count>` files matching
    /// `<prefix>_*` exist in `<dir>`.
    Barrier {
        dir: String,
        prefix: String,
        count: usize,
        /// Exit with the given code after the barrier is met.
        #[arg(long = "exit", default_value_t = 0)]
        exit_code: i32,
        /// Keep process alive after the barrier (for kill tests).
        #[arg(long)]
        hang: bool,
        /// Close stdout/stderr but keep process alive (for daemon kill tests).
        #[arg(long)]
        daemonize: bool,
    },
    /// Print whether stdin/stdout/stderr are a TTY.
    CheckTty,
    /// Print the given arguments joined by spaces.
    Print {
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Print the current working directory.
    PrintCwd,
    /// Print the value of an environment variable.
    PrintEnv { var_name: String },
    /// Print the contents of one or more files.
    PrintFile {
        #[arg(trailing_var_arg = true)]
        files: Vec<String>,
    },
    /// Echo stdin to stdout.
    ReadStdin,
    /// Replace the first occurrence of a search value in a file.
    ReplaceFileContent { filename: String, search_value: String, new_value: String },
    /// Update a file's mtime (file must exist).
    TouchFile { filename: String },
}

fn main() {
    let cli = Cli::parse();
    let result: Result<(), Box<dyn std::error::Error>> = match cli.command {
        Commands::Barrier { dir, prefix, count, exit_code, hang, daemonize } => {
            cmd_barrier(&dir, &prefix, count, exit_code, hang, daemonize)
        }
        Commands::CheckTty => {
            cmd_check_tty();
            Ok(())
        }
        Commands::Print { args } => {
            cmd_print(&args);
            Ok(())
        }
        Commands::PrintCwd => cmd_print_cwd(),
        Commands::PrintEnv { var_name } => {
            cmd_print_env(&var_name);
            Ok(())
        }
        Commands::PrintFile { files } => cmd_print_file(&files),
        Commands::ReadStdin => cmd_read_stdin(),
        Commands::ReplaceFileContent { filename, search_value, new_value } => {
            cmd_replace_file_content(&filename, &search_value, &new_value)
        }
        Commands::TouchFile { filename } => cmd_touch_file(&filename),
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn cmd_barrier(
    dir: &str,
    prefix: &str,
    count: usize,
    exit_code: i32,
    hang: bool,
    daemonize: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use notify::Watcher as _;

    let dir = std::path::Path::new(dir);
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

fn cmd_print_env(var_name: &str) {
    let value = std::env::var(var_name).unwrap_or_else(|_| "(undefined)".to_string());
    println!("{value}");
}

fn cmd_print_file(files: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for file in files {
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

fn cmd_replace_file_content(
    filename: &str,
    search_value: &str,
    new_value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let filepath = std::path::Path::new(filename).canonicalize()?;
    let content = std::fs::read_to_string(&filepath)?;
    if !content.contains(search_value) {
        return Err(std::format!("searchValue not found in {filename}: {search_value:?}").into());
    }
    let new_content = content.replacen(search_value, new_value, 1);
    std::fs::write(&filepath, new_content)?;
    Ok(())
}

fn cmd_touch_file(filename: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _file = std::fs::OpenOptions::new().read(true).write(true).open(filename)?;
    Ok(())
}

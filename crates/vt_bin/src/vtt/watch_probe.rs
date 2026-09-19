use std::{
    fs::OpenOptions,
    io::Write as _,
    net::TcpListener,
    process::{Command, Stdio},
};

/// A deterministic watch-test command: reads an input, records each invocation,
/// and optionally stays alive with a descendant owning a TCP port.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.first().is_some_and(|arg| arg == "--child") {
        let _listener = TcpListener::bind(("127.0.0.1", args[2].parse::<u16>()?))?;
        pty_terminal_test_client::mark_milestone(&args[1]);
        loop {
            std::thread::park();
        }
    }
    let value = if args.get(2).is_some_and(|mode| mode == "directory") {
        let mut entries = std::fs::read_dir(&args[0])?
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        if entries.is_empty() { "empty".into() } else { entries.join("-") }
    } else {
        std::fs::read_to_string(&args[0]).unwrap_or_else(|_| "missing".into())
    };
    let value = value.trim();
    let mut output = OpenOptions::new().create(true).append(true).open(&args[1])?;
    writeln!(output, "{value}")?;
    output.flush()?;
    println!("input={value}");
    if args.get(2).is_some_and(|mode| mode == "tree") {
        let mut child = Command::new(std::env::current_exe()?)
            .args(["watch-probe", "--child", value, &args[3]])
            .stdin(Stdio::null())
            .spawn()?;
        let status = child.wait()?;
        if !status.success() {
            return Err("watch descendant failed".into());
        }
        return Ok(());
    }
    pty_terminal_test_client::mark_milestone(value);
    if args.get(2).is_some_and(|mode| mode == "gate") {
        while !std::path::Path::new(&args[3]).exists() {
            // This file is a test barrier, not a presumed execution duration.
            #[expect(
                clippy::disallowed_methods,
                reason = "wait for an explicit test barrier file"
            )]
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    if value == "fail" {
        std::process::exit(2);
    }
    if args.get(2).is_some_and(|mode| mode == "server") {
        loop {
            std::thread::park();
        }
    }
    Ok(())
}

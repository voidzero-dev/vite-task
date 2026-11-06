/// pipe-stdout -- `<command>` \[`<args>`...\]
///
/// Spawns `<command>` with its stdout piped instead of the terminal, then prints what
/// it wrote there after a `--- stdout ---` line and exits with the child's exit code.
/// Its stdin and stderr stay connected to the terminal.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [separator, program, args @ ..] = args else {
        return Err("Usage: vtt pipe-stdout -- <command> [args...]".into());
    };
    if separator != "--" {
        return Err("Usage: vtt pipe-stdout -- <command> [args...]".into());
    }

    let output = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()?;

    {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(b"--- stdout ---\n")?;
        stdout.write_all(&output.stdout)?;
        stdout.flush()?;
    }
    std::process::exit(output.status.code().unwrap_or(1));
}

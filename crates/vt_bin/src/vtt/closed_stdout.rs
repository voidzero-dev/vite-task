/// closed-stdout -- `<command>` \[`<args>`...\]
///
/// Spawns `<command>` with its stdout connected to a pipe whose reading end is
/// already closed, so every write to its stdout fails, and exits with the
/// child's exit code. Its stdin and stderr stay connected to the terminal.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [separator, program, args @ ..] = args else {
        return Err("Usage: vtt closed-stdout -- <command> [args...]".into());
    };
    if separator != "--" {
        return Err("Usage: vtt closed-stdout -- <command> [args...]".into());
    }

    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::inherit())
        .stdout(writer)
        .stderr(std::process::Stdio::inherit())
        .status()?;
    std::process::exit(status.code().unwrap_or(1));
}

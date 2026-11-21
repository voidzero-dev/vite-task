/// broken-stdout -- `<command>` \[`<args>`...\]
///
/// Spawns `<command>` with its stdout connected to a pipe whose read end is
/// already closed, so every write to it fails (`EPIPE` on Unix,
/// `ERROR_NO_DATA` on Windows), then exits with the child's exit code.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let Some(("--", cmd_args)) = args.split_first().map(|(sep, rest)| (sep.as_str(), rest)) else {
        return Err("Usage: vtt broken-stdout -- <command> [args...]".into());
    };
    let Some((program, program_args)) = cmd_args.split_first() else {
        return Err("Usage: vtt broken-stdout -- <command> [args...]".into());
    };

    let (reader, writer) = std::io::pipe()?;
    drop(reader);
    let status = std::process::Command::new(program).args(program_args).stdout(writer).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

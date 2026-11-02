use std::{fs, io};

const USAGE: &str = "Usage: vtt fs <op> <arg>... [<op> <arg>...]...";

/// fs `<op>` `<arg>`... \[`<op>` `<arg>`...\]...
///
/// Makes a sequence of file system calls in one process, in order. vt caches
/// each part of an `&&` command on its own, so an access pattern that spans
/// several calls, such as reading a file and writing it back, has to stay in
/// one process. Each operation is its name followed by a fixed number of
/// arguments:
///
/// - `read <file>`: read the file. A missing file isn't an error, like a tool
///   probing for its cache.
/// - `write <file> <content>`: create or truncate the file and write
///   `<content>`. The parent folder must exist.
/// - `stat <path>`: read the path's metadata, following symlinks. A missing
///   path isn't an error.
/// - `list <dir>`: read every entry of the folder. A missing folder isn't an
///   error.
/// - `mkdir <dir>`: create the folder and any missing parents, like
///   `mkdir -p`.
///
/// Prints nothing. The first call that fails stops the sequence with an error
/// naming the operation.
///
/// Example: `vtt fs read state.txt write state.txt v1`
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if args.is_empty() {
        return Err(USAGE.into());
    }
    let mut ops = args.as_slice();
    while !ops.is_empty() {
        let (result, rest) = match ops {
            ["read", file, rest @ ..] => (allow_missing(fs::read(file)), rest),
            ["write", file, content, rest @ ..] => (fs::write(file, content), rest),
            ["stat", path, rest @ ..] => (allow_missing(fs::metadata(path)), rest),
            ["list", dir, rest @ ..] => (allow_missing(list(dir)), rest),
            ["mkdir", dir, rest @ ..] => (fs::create_dir_all(dir), rest),
            _ => return Err(format!("{USAGE}\nunexpected arguments: {}", ops.join(" ")).into()),
        };
        let op = &ops[..ops.len() - rest.len()];
        result.map_err(|err| format!("vtt fs {}: {err}", op.join(" ")))?;
        ops = rest;
    }
    Ok(())
}

fn list(dir: &str) -> io::Result<()> {
    fs::read_dir(dir)?.try_for_each(|entry| entry.map(drop))
}

fn allow_missing<T>(result: io::Result<T>) -> io::Result<()> {
    match result {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

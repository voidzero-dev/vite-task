use std::{error::Error, io};

const USAGE: &str = "Usage: vtt stat_long_filename <count>";

pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let count = parse_count(args)?;
    access_generated_path(count, metadata)?;
    Ok(())
}

fn parse_count(args: &[String]) -> Result<usize, String> {
    let [count] = args else { return Err(USAGE.to_owned()) };
    count.parse().map_err(|_| USAGE.to_owned())
}

fn generated_path(count: usize) -> String {
    "x".repeat(count)
}

fn access_generated_path(
    count: usize,
    mut metadata: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let path = generated_path(count);
    match metadata(&path) {
        Ok(()) => Ok(()),
        Err(error) if is_absent_or_too_long(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether the platform said the file is not there, or that the name is
/// longer than it accepts. Either is the expected answer: this command
/// exists to have the access attempted and recorded, not to find a file.
///
/// Windows reports an over-long name as `ERROR_FILENAME_EXCED_RANGE`,
/// which reaches here as [`io::ErrorKind::InvalidFilename`] rather than as
/// the `ENAMETOOLONG` unix returns.
fn is_absent_or_too_long(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::InvalidFilename)
        || error.raw_os_error() == Some(libc::ENAMETOOLONG)
}

fn metadata(path: &str) -> io::Result<()> {
    std::fs::metadata(path).map(|_| ())
}

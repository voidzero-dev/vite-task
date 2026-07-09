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
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ENAMETOOLONG) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn metadata(path: &str) -> io::Result<()> {
    std::fs::metadata(path).map(|_| ())
}

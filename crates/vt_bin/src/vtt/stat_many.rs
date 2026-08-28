//! Stats generated names, to make a known number of tracked file accesses.
//!
//! The names are missing on purpose: an access is recorded whether or not
//! the file is there, and this exists to have the access attempted, not to
//! find a file. Each name differs from the last, so no two can fold into
//! one record.
//!
//! Both knobs give tracking more than it can hold, and only one of them
//! travels. A count works everywhere. A long name does not: on Windows a
//! path reaches the tracer through a `UNICODE_STRING`, whose length is a
//! `u16`, so however long a name this asks for, no single record there
//! exceeds 64 KiB.

use std::{error::Error, io};

const USAGE: &str = "Usage: vtt stat-many <count> [name-length]";

pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let (count, name_length) = parse_args(args)?;
    for index in 0..count {
        access_generated_path(index, name_length, metadata)?;
    }
    // Printing last proves the process survived every one of them, which a
    // channel that has stopped accepting records must not disturb.
    println!("stat {count}");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<(usize, usize), String> {
    let (count, name_length) = match args {
        [count] => (count, None),
        [count, name_length] => (count, Some(name_length)),
        _ => return Err(USAGE.to_owned()),
    };
    let count = count.parse().map_err(|_| USAGE.to_owned())?;
    let name_length =
        name_length.map(|length| length.parse()).transpose().map_err(|_| USAGE.to_owned())?;
    Ok((count, name_length.unwrap_or(0)))
}

/// A name unique to `index`, padded out to `name_length` when that leaves
/// room for padding. A length short enough to truncate the index would
/// hand two accesses the same name, so the index always survives.
fn generated_path(index: usize, name_length: usize) -> String {
    let name = std::format!("vtt-stat-many-{index}");
    let padding = name_length.saturating_sub(name.len());
    name + &"x".repeat(padding)
}

fn access_generated_path(
    index: usize,
    name_length: usize,
    mut metadata: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let path = generated_path(index, name_length);
    match metadata(&path) {
        Ok(()) => Ok(()),
        Err(error) if is_absent_or_too_long(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether the platform said the file is not there, or that the name is
/// longer than it accepts. Either is the expected answer.
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

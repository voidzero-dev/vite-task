//! Stats a run of generated names, to make a known number of tracked file
//! accesses. The names are missing on purpose, and each one differs from
//! the last: an access is recorded whether or not the file is there, and
//! distinct names cannot be folded into one record.
//!
//! A count is the portable way to give tracking more than it can hold.
//! Record size is not: on Windows a path arrives through a
//! `UNICODE_STRING`, whose length is a `u16`, so no single record there can
//! exceed 64 KiB however long a name the caller asks for.

use std::error::Error;

const USAGE: &str = "Usage: vtt stat-many <count>";

pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let [count] = args else { return Err(USAGE.into()) };
    let count: usize = count.parse().map_err(|_| USAGE)?;
    for index in 0..count {
        let _ = std::fs::metadata(format!("vtt-stat-many-{index}"));
    }
    // Printing last proves the process survived every one of them, which is
    // what a channel that has stopped accepting records must not disturb.
    println!("stat {count}");
    Ok(())
}

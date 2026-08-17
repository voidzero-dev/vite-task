//! Stats a run of generated names, to make a known number of tracked file
//! accesses. The names are missing on purpose: an access is recorded
//! whether or not the file is there, and nothing is left behind.

use std::error::Error;

const USAGE: &str = "Usage: vtt stat-many <count>";

pub fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let [count] = args else { return Err(USAGE.into()) };
    let count: usize = count.parse().map_err(|_| USAGE)?;
    for index in 0..count {
        let _ = std::fs::metadata(format!("vtt-stat-many-{index}"));
    }
    // Printing last proves the process survived every one of them, which
    // is what a channel that stops accepting records must not disturb.
    println!("stat {count}");
    Ok(())
}

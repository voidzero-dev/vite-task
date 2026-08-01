//! Publish an output directory the way real build tools do, in one process.
//!
//! Both modes exist because they stress different parts of auto-output tracking,
//! and both must happen inside a single task: a compound `a && b` command is
//! cached as separate tasks, so splitting the steps would hide the very
//! relationship under test.
//!
//! `atomic` stages the output under a temporary directory and renames that
//! directory into place. Every write lands on the staging path and only the
//! directory carries the rename, so a tracker that ignores directory renames
//! collects nothing.
//!
//! `rebuild` empties the output directory first, which is what makes a build
//! tool stat and list its own output. Those reads must not become inputs.

use std::path::Path;

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [mode, source, directory] = args else {
        return Err("Usage: vtt publish-dir <atomic|rebuild> <source> <directory>".into());
    };
    let directory = Path::new(directory);
    let contents = std::fs::read_to_string(source)?;

    match mode.as_str() {
        "atomic" => {
            let staging = directory.with_extension("tmp");
            if staging.exists() {
                std::fs::remove_dir_all(&staging)?;
            }
            std::fs::create_dir_all(&staging)?;
            std::fs::write(staging.join("out.txt"), &contents)?;
            if directory.exists() {
                std::fs::remove_dir_all(directory)?;
            }
            std::fs::rename(&staging, directory)?;
        }
        "rebuild" => {
            if directory.exists() {
                std::fs::remove_dir_all(directory)?;
            }
            std::fs::create_dir_all(directory)?;
            std::fs::write(directory.join("out.txt"), &contents)?;
        }
        other => return Err(format!("unknown mode: {other}").into()),
    }
    Ok(())
}

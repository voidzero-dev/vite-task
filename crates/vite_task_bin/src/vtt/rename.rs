//! Rename a file or a whole directory, the way tools publish results atomically.
//!
//! Build tools commonly stage output under a temporary name and rename it into
//! place so readers never see a half-written tree. Renaming a *directory* is the
//! interesting case for tracking: every write lands on the staging path, and only
//! the directory itself carries the rename, so a tracker that ignores directory
//! renames loses every output.

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 2 {
        return Err("Usage: vtt rename <from> <to>".into());
    }
    std::fs::rename(&args[0], &args[1])?;
    Ok(())
}

/// List entries in a directory, sorted by name, one per line.
///
/// Usage: `vtt list-dir <dir> [--ext <suffix>] [--recursive]`
///
/// With `--ext`, only entries whose filename ends with the given suffix are
/// printed (the leading `.` is part of the suffix you pass, e.g. `.tar.zst`).
///
/// With `--recursive`, subdirectories are walked and only their leaf files are
/// printed (by basename, not path). This lets tests assert on cache contents
/// without hardcoding the per-schema-version subdirectory (e.g. `v13`) that the
/// cache database and archives live under, which changes whenever the cache
/// schema version is bumped.
///
/// Used by e2e tests to assert on cache directory contents (e.g. exactly one
/// `.tar.zst` archive after a re-run that should have cleaned up the prior
/// archive).
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut dir: Option<&str> = None;
    let mut ext: Option<&str> = None;
    let mut recursive = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ext" => {
                i += 1;
                ext = Some(args.get(i).ok_or("--ext requires a value")?.as_str());
            }
            "--recursive" => recursive = true,
            other if dir.is_none() => dir = Some(other),
            other => return Err(format!("unexpected argument: {other}").into()),
        }
        i += 1;
    }
    let dir = dir.ok_or("Usage: vtt list-dir <dir> [--ext <suffix>] [--recursive]")?;

    let mut names: Vec<String> = Vec::new();
    collect(std::path::Path::new(dir), ext, recursive, &mut names)?;
    names.sort();
    for name in names {
        println!("{name}");
    }
    Ok(())
}

/// Collect entry filenames under `dir`. In recursive mode, descend into
/// subdirectories and collect only leaf files (the directory names themselves
/// are not emitted).
fn collect(
    dir: &std::path::Path,
    ext: Option<&str>,
    recursive: bool,
    names: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if recursive && entry.file_type()?.is_dir() {
            collect(&entry.path(), ext, recursive, names)?;
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(suffix) = ext
            && !name.ends_with(suffix)
        {
            continue;
        }
        names.push(name);
    }
    Ok(())
}

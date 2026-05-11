/// List entries in a directory, sorted by name, one per line.
///
/// Usage: `vtt list-dir <dir> [--ext <suffix>]`
///
/// With `--ext`, only entries whose filename ends with the given suffix are
/// printed (the leading `.` is part of the suffix you pass, e.g. `.tar.zst`).
///
/// Used by e2e tests to assert on cache directory contents (e.g. exactly one
/// `.tar.zst` archive after a re-run that should have cleaned up the prior
/// archive).
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut dir: Option<&str> = None;
    let mut ext: Option<&str> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ext" => {
                i += 1;
                ext = Some(args.get(i).ok_or("--ext requires a value")?.as_str());
            }
            other if dir.is_none() => dir = Some(other),
            other => return Err(format!("unexpected argument: {other}").into()),
        }
        i += 1;
    }
    let dir = dir.ok_or("Usage: vtt list-dir <dir> [--ext <suffix>]")?;

    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(suffix) = ext
            && !name.ends_with(suffix)
        {
            continue;
        }
        names.push(name);
    }
    names.sort();
    for name in names {
        println!("{name}");
    }
    Ok(())
}

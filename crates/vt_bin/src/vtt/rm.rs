/// Remove files and directories.
///
/// Usage: `vtt rm [-rf] <path>...` or `vtt rm --ext <suffix> <dir>`
///
/// With `--ext`, every file under `<dir>`, recursively, whose name ends with
/// `<suffix>` is removed instead. Like `vtt list-dir --recursive`, this lets
/// tests remove cache archives without hardcoding their names or the
/// per-schema-version subdirectory they live under.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if let [flag, suffix, dir] = args
        && flag == "--ext"
    {
        return remove_with_suffix(std::path::Path::new(dir), suffix);
    }
    let mut recursive = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-r" | "-rf" | "-f" => recursive = true,
            _ => paths.push(arg.as_str()),
        }
    }
    if paths.is_empty() {
        return Err("Usage: vtt rm [-rf] <path>... | vtt rm --ext <suffix> <dir>".into());
    }
    for path in paths {
        let p = std::path::Path::new(path);
        if p.is_dir() && recursive {
            std::fs::remove_dir_all(p)?;
        } else {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

fn remove_with_suffix(
    dir: &std::path::Path,
    suffix: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_with_suffix(&entry.path(), suffix)?;
        } else if entry.file_name().to_string_lossy().ends_with(suffix) {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut recursive = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-r" | "-rf" | "-f" => recursive = true,
            _ => paths.push(arg.as_str()),
        }
    }
    if paths.is_empty() {
        return Err("Usage: vtt rm [-rf] <path>...".into());
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

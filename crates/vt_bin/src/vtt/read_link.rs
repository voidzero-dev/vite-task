/// Print the target of the symbolic link at `<path>`, like `readlink`. Fails if
/// `<path>` isn't a symbolic link.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path] = args else {
        return Err("Usage: vtt read-link <path>".into());
    };
    println!("{}", std::fs::read_link(path)?.display());
    Ok(())
}

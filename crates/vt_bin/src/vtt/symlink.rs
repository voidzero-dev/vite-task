/// Create a symbolic link at `<link>` that points to `<target>`, like `ln -s`.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [target, link] = args else {
        return Err("Usage: vtt symlink <target> <link>".into());
    };
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

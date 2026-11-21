/// Create a symbolic link at `<link>` that points to `<target>`, like `ln -s`.
/// On Windows, the link is a directory link if `<target>`, resolved from the
/// link's folder, is a directory.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [target, link] = args else {
        return Err("Usage: vtt symlink <target> <link>".into());
    };
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    #[cfg(windows)]
    {
        let link = std::path::Path::new(link);
        if link.parent().is_some_and(|parent| parent.join(target).is_dir()) {
            std::os::windows::fs::symlink_dir(target, link)?;
        } else {
            std::os::windows::fs::symlink_file(target, link)?;
        }
    }
    Ok(())
}

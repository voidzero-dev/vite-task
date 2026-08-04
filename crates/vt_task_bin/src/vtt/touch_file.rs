pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return Err("Usage: vtt touch-file <filename>".into());
    }
    let _file = std::fs::OpenOptions::new().read(true).write(true).open(&args[0])?;
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() < 2 {
        return Err("Usage: vtt write-file <filename> <content>".into());
    }
    std::fs::write(&args[0], &args[1])?;
    Ok(())
}

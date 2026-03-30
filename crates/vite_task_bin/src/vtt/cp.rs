pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 2 {
        return Err("Usage: vtt cp <src> <dst>".into());
    }
    std::fs::copy(&args[0], &args[1])?;
    Ok(())
}

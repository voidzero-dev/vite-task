pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for file in args {
        match std::fs::read(file) {
            Ok(content) => out.write_all(&content)?,
            Err(_) => eprintln!("{file}: not found"),
        }
    }
    Ok(())
}

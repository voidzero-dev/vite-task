pub fn run(args: &[String]) {
    let [path, pattern] = args else {
        eprintln!("Usage: vtt grep-file <path> <pattern>");
        std::process::exit(2);
    };
    match std::fs::read_to_string(path) {
        Ok(content) => {
            if content.contains(pattern.as_str()) {
                println!("{path}: found {pattern:?}");
            } else {
                println!("{path}: missing {pattern:?}");
            }
        }
        Err(_) => println!("{path}: not found"),
    }
}

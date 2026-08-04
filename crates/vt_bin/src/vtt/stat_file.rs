pub fn run(args: &[String]) {
    for file in args {
        if std::fs::metadata(file).is_ok() {
            println!("{file}: exists");
        } else {
            println!("{file}: missing");
        }
    }
}

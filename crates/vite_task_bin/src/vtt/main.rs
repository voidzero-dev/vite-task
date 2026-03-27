// This is a standalone test utility binary that deliberately uses std types
// rather than the project's custom types (vite_str, vite_path, etc.).
#![expect(clippy::disallowed_types, reason = "standalone test utility uses std types")]
#![expect(clippy::disallowed_macros, reason = "standalone test utility uses std macros")]
#![expect(clippy::disallowed_methods, reason = "standalone test utility uses std methods")]
#![expect(clippy::print_stderr, reason = "CLI tool error output")]
#![expect(clippy::print_stdout, reason = "CLI tool output")]

mod barrier;
mod check_tty;
mod print;
mod print_cwd;
mod print_env;
mod print_file;
mod read_stdin;
mod replace_file_content;
mod touch_file;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vtt <subcommand> [args...]");
        eprintln!(
            "Subcommands: barrier, check-tty, print, print-cwd, print-env, print-file, read-stdin, replace-file-content, touch-file"
        );
        std::process::exit(1);
    }

    let result: Result<(), Box<dyn std::error::Error>> = match args[1].as_str() {
        "barrier" => barrier::run(&args[2..]),
        "check-tty" => {
            check_tty::run();
            Ok(())
        }
        "print" => {
            print::run(&args[2..]);
            Ok(())
        }
        "print-cwd" => print_cwd::run(),
        "print-env" => print_env::run(&args[2..]),
        "print-file" => print_file::run(&args[2..]),
        "read-stdin" => read_stdin::run(),
        "replace-file-content" => replace_file_content::run(&args[2..]),
        "touch-file" => touch_file::run(&args[2..]),
        other => {
            eprintln!("Unknown subcommand: {other}");
            std::process::exit(1);
        }
    };

    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

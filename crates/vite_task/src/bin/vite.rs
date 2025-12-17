use clap::Parser;
use vite_str::Str;
use vite_task::CLIArgs;

#[derive(Debug, Parser)]
enum CustomTaskSubCommand {
    /// oxlint
    Lint { args: Vec<Str> },
}

fn main() {
    let _subcommand = CLIArgs::<CustomTaskSubCommand>::parse();
}

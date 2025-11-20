use clap::{Parser, Subcommand};
use vite_str::Str;

#[derive(Debug, Parser)]
pub enum CLIArgs<SubCommands: Subcommand> {
    #[clap(flatten)]
    SubCommands(SubCommands),

    Run {
        #[clap(short, long)]
        recursive: bool,
        task_name: Str,
        #[clap(last = true)]
        extra_args: Vec<Str>,
    },
}

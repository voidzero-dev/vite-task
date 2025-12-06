use clap::Parser;
use vite_str::Str;

#[derive(Parser)]
enum SubCommand {
    /// Run tasks
    Run {
        #[clap(flatten)]
        query: vite_task_graph::query::cli::CLITaskQuery,

        /// Additional arguments to pass to the tasks
        #[clap(last = true)]
        args: Vec<Str>,
    },
}

fn main() {
    let _subcommand = SubCommand::parse();
}

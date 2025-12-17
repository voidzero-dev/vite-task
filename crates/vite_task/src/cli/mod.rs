use clap::{Parser, Subcommand};
use vite_str::Str;
use vite_task_graph::TaskSpecifier;

/// The CLI arguments for vite task, with customizable subcommands.
#[derive(Debug, clap::Parser)]
pub struct CLIArgs<CustomSubCommand: Subcommand> {
    inner: CLIArgsInner<CustomSubCommand>,
}

#[derive(Debug, clap::Parser)]
pub enum CLIArgsInner<CustomSubCommand: Subcommand> {
    /// subcommands provided by vite task
    #[clap(flatten)]
    ViteTask(ViteTaskSubCommand),

    /// custom subcommands provided by vite+
    #[clap(flatten)]
    Custom(CustomSubCommand),
}

/// vite task CLI subcommands
#[derive(Debug, Parser)]
pub enum ViteTaskSubCommand {
    Run {
        /// `packageName#taskName` or `taskName`.
        task_specifier: TaskSpecifier,

        /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
        #[clap(default_value = "false", short, long)]
        recursive: bool,

        /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
        #[clap(default_value = "false", short, long)]
        transitive: bool,

        /// Do not run dependencies specified in `dependsOn` fields.
        #[clap(default_value = "false", long)]
        ignore_depends_on: bool,

        /// Additional arguments to pass to the tasks
        #[clap(trailing_var_arg = true)]
        args: Vec<Str>,
    },
}

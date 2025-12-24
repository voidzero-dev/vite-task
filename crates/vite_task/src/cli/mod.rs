use std::{ffi::OsStr, sync::Arc};

use clap::{Parser, Subcommand};
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskSpecifier, query::TaskQueryKind};
use vite_task_plan::plan_request::{PlanOptions, PlanRequest, QueryPlanRequest};

/// Represents the CLI arguments handled by vite-task, including both built-in and custom subcommands.
#[derive(Debug)]
pub struct TaskCLIArgs<CustomSubCommand: Subcommand> {
    pub(crate) original: Arc<[Str]>,
    pub(crate) parsed: ParsedTaskCLIArgs<CustomSubCommand>,
}

pub enum CLIArgs<CustomSubCommand: Subcommand, NonTaskSubCommand: Subcommand> {
    /// vite-task's own built-in subcommands
    Task(TaskCLIArgs<CustomSubCommand>),
    /// custom subcommands provided by vite+
    NonTask(NonTaskSubCommand),
}

impl<CustomSubCommand: Subcommand, NonTaskSubCommand: Subcommand>
    CLIArgs<CustomSubCommand, NonTaskSubCommand>
{
    /// Get the original CLI arguments
    pub fn try_parse_from(
        args: impl Iterator<Item = impl AsRef<str>>,
    ) -> Result<Self, clap::Error> {
        #[derive(Debug, clap::Parser)]
        enum ParsedCLIArgs<CustomSubCommand: Subcommand, NonTaskSubCommand: Subcommand> {
            /// subcommands handled by vite task
            #[command(flatten)]
            Task(ParsedTaskCLIArgs<CustomSubCommand>),

            /// subcommands that are not handled by vite task
            #[command(flatten)]
            NonTask(NonTaskSubCommand),
        }

        let args = args.map(|arg| Str::from(arg.as_ref())).collect::<Arc<[Str]>>();
        let parsed_cli_args = ParsedCLIArgs::<CustomSubCommand, NonTaskSubCommand>::try_parse_from(
            args.iter().map(|s| OsStr::new(s.as_str())),
        )?;

        Ok(match parsed_cli_args {
            ParsedCLIArgs::Task(parsed_task_cli_args) => {
                Self::Task(TaskCLIArgs { original: args, parsed: parsed_task_cli_args })
            }
            ParsedCLIArgs::NonTask(non_task_subcommand) => Self::NonTask(non_task_subcommand),
        })
    }
}

#[derive(Debug, Parser)]
pub(crate) enum ParsedTaskCLIArgs<CustomSubCommand: Subcommand> {
    /// subcommands provided by vite task, like `run`
    #[clap(flatten)]
    BuiltIn(BuiltInCommand),
    /// custom subcommands provided by vite+, like `lint`
    #[clap(flatten)]
    Custom(CustomSubCommand),
}

/// vite task CLI subcommands
#[derive(Debug, Subcommand)]
pub(crate) enum BuiltInCommand {
    /// Run tasks
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
        additional_args: Vec<Str>,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --recursive")]
    PackageNameSpecifiedWithRecursive { package_name: Str, task_name: Str },
}

impl BuiltInCommand {
    /// Convert to `TaskQuery`, or return an error if invalid.
    pub fn into_plan_request(
        self,
        cwd: &Arc<AbsolutePath>,
    ) -> Result<PlanRequest, CLITaskQueryError> {
        match self {
            Self::Run {
                task_specifier,
                recursive,
                transitive,
                ignore_depends_on,
                additional_args,
            } => {
                let include_explicit_deps = !ignore_depends_on;

                let query_kind = if recursive {
                    if transitive {
                        return Err(CLITaskQueryError::RecursiveTransitiveConflict);
                    }
                    let task_name = if let Some(package_name) = task_specifier.package_name {
                        return Err(CLITaskQueryError::PackageNameSpecifiedWithRecursive {
                            package_name,
                            task_name: task_specifier.task_name,
                        });
                    } else {
                        task_specifier.task_name
                    };
                    TaskQueryKind::Recursive { task_names: [task_name].into() }
                } else {
                    TaskQueryKind::Normal {
                        task_specifiers: [task_specifier].into(),
                        cwd: Arc::clone(cwd),
                        include_topological_deps: transitive,
                    }
                };
                Ok(PlanRequest::Query(QueryPlanRequest {
                    query: vite_task_graph::query::TaskQuery {
                        kind: query_kind,
                        include_explicit_deps,
                    },
                    plan_options: PlanOptions { extra_args: additional_args.into() },
                }))
            }
        }
    }
}

use std::sync::Arc;

use clap::Parser;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskSpecifier, query::TaskQueryKind};
use vite_task_plan::plan_request::{PlanOptions, PlanRequest, QueryPlanRequest};

#[derive(Debug, Clone, clap::Subcommand)]
pub enum CacheSubcommand {
    /// Clean up all the cache
    Clean,
}

/// Arguments for the `run` subcommand.
#[derive(Debug, clap::Args)]
pub struct RunCommand {
    /// `packageName#taskName` or `taskName`.
    pub task_specifier: TaskSpecifier,

    /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub recursive: bool,

    /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub transitive: bool,

    /// Do not run dependencies specified in `dependsOn` fields.
    #[clap(default_value = "false", long)]
    pub ignore_depends_on: bool,

    /// Additional arguments to pass to the tasks
    #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
    pub additional_args: Vec<Str>,
}

/// vite task CLI subcommands
#[derive(Debug, Parser)]
pub enum Command {
    /// Run tasks
    Run(RunCommand),
    /// Manage the task cache
    Cache {
        #[clap(subcommand)]
        subcmd: CacheSubcommand,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --recursive")]
    PackageNameSpecifiedWithRecursive { package_name: Str, task_name: Str },
}

impl RunCommand {
    /// Convert to `PlanRequest`, or return an error if invalid.
    pub fn into_plan_request(
        self,
        cwd: &Arc<AbsolutePath>,
    ) -> Result<PlanRequest, CLITaskQueryError> {
        let RunCommand {
            task_specifier,
            recursive,
            transitive,
            ignore_depends_on,
            additional_args,
        } = self;

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
            query: vite_task_graph::query::TaskQuery { kind: query_kind, include_explicit_deps },
            plan_options: PlanOptions { extra_args: additional_args.into() },
        }))
    }
}

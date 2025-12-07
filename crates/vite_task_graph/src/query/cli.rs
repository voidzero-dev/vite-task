use std::{collections::HashSet, sync::Arc};

use vite_path::AbsolutePath;
use vite_str::Str;

use super::TaskQueryKind;
use crate::{query::TaskQuery, specifier::TaskSpecifier};

/// Represents task query args of `vite run`
/// It will be converted to `TaskQuery`, but may be invalid (contains conflicting options),
/// if so the error is returned early before loading the task graph.
#[derive(Debug, clap::Parser)]
pub struct CLITaskQuery {
    /// Specifies one or multiple tasks to run, in form of `packageName#taskName` or `taskName`.
    tasks: Vec<TaskSpecifier>,

    /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    recursive: bool,

    /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    transitive: bool,

    /// Do not run dependencies specified in `dependsOn` fields.
    #[clap(default_value = "false", long)]
    ignore_depends_on: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --recursive")]
    PackageNameSpecifiedWithRecursive { package_name: Str, task_name: Str },
}

impl CLITaskQuery {
    /// Convert to `TaskQuery`, or return an error if invalid.
    pub fn into_task_query(self, cwd: &Arc<AbsolutePath>) -> Result<TaskQuery, CLITaskQueryError> {
        let include_explicit_deps = !self.ignore_depends_on;

        let kind = if self.recursive {
            if self.transitive {
                return Err(CLITaskQueryError::RecursiveTransitiveConflict);
            }
            let task_names: HashSet<Str> = self
                .tasks
                .into_iter()
                .map(|s| {
                    if let Some(package_name) = s.package_name {
                        return Err(CLITaskQueryError::PackageNameSpecifiedWithRecursive {
                            package_name,
                            task_name: s.task_name,
                        });
                    }
                    Ok(s.task_name)
                })
                .collect::<Result<_, _>>()?;
            TaskQueryKind::Recursive { task_names }
        } else {
            TaskQueryKind::Normal {
                task_specifiers: self.tasks.into_iter().collect(),
                cwd: Arc::clone(cwd),
                include_topological_deps: self.transitive,
            }
        };
        Ok(TaskQuery { kind, include_explicit_deps })
    }
}

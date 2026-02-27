use std::sync::Arc;

use clap::Parser;
use vec1::Vec1;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{
    TaskSpecifier,
    query::{PackageQuery, TaskQuery},
};
use vite_task_plan::plan_request::{PlanOptions, QueryPlanRequest};
use vite_workspace::package_filter::{
    GraphTraversal, PackageFilter, PackageFilterParseError, PackageNamePattern, PackageSelector,
    TraversalDirection, parse_filter,
};

#[derive(Debug, Clone, clap::Subcommand)]
pub enum CacheSubcommand {
    /// Clean up all the cache
    Clean,
}

/// Flags that control how a `run` command selects tasks.
#[derive(Debug, Clone, clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "CLI flags are naturally boolean")]
pub struct RunFlags {
    /// Run tasks found in all packages in the workspace, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub recursive: bool,

    /// Run tasks found in the current package and all its transitive dependencies, in topological order based on package dependencies.
    #[clap(default_value = "false", short, long)]
    pub transitive: bool,

    /// Do not run dependencies specified in `dependsOn` fields.
    #[clap(default_value = "false", long)]
    pub ignore_depends_on: bool,

    /// Show full detailed summary after execution.
    #[clap(default_value = "false", short = 'v', long)]
    pub verbose: bool,

    /// Filter packages (pnpm --filter syntax). Can be specified multiple times.
    #[clap(short = 'F', long, num_args = 1)]
    pub filter: Vec<Str>,
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Public CLI types (clap-parsed)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Arguments for the `run` subcommand as parsed by clap.
///
/// Contains the `--last-details` flag which is resolved into a separate
/// `ResolvedCommand::RunLastDetails` variant internally.
#[derive(Debug, clap::Args)]
pub struct RunCommand {
    /// `packageName#taskName` or `taskName`. If omitted, lists all available tasks.
    pub(crate) task_specifier: Option<TaskSpecifier>,

    #[clap(flatten)]
    pub(crate) flags: RunFlags,

    /// Additional arguments to pass to the tasks
    #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
    pub(crate) additional_args: Vec<Str>,

    /// Display the detailed summary of the last run.
    #[clap(long, exclusive = true)]
    pub(crate) last_details: bool,
}

/// vite task CLI subcommands as parsed by clap.
///
/// vite task CLI subcommands as parsed by clap.
///
/// Pass directly to `Session::main` or `HandledCommand::ViteTaskCommand`.
/// The `--last-details` flag on the `run` subcommand is resolved internally.
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

impl Command {
    /// Resolve the clap-parsed command into the dispatched [`ResolvedCommand`] enum.
    ///
    /// When `--last-details` is set on the `run` subcommand, this produces
    /// [`ResolvedCommand::RunLastDetails`] instead of [`ResolvedCommand::Run`],
    /// making the exclusivity enforced at the type level.
    #[must_use]
    pub(crate) fn into_resolved(self) -> ResolvedCommand {
        match self {
            Self::Run(run) if run.last_details => ResolvedCommand::RunLastDetails,
            Self::Run(run) => ResolvedCommand::Run(run.into_resolved()),
            Self::Cache { subcmd } => ResolvedCommand::Cache { subcmd },
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Internal resolved types (used for dispatch — `--last-details` is a separate variant)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Resolved CLI command for internal dispatch.
///
/// Unlike [`Command`], this enum makes `--last-details` a separate variant
/// ([`ResolvedCommand::RunLastDetails`]) so that it is exclusive at the type level —
/// there is no way to combine it with task execution fields.
#[derive(Debug)]
pub enum ResolvedCommand {
    /// Run tasks with the given parameters.
    Run(ResolvedRunCommand),
    /// Display the saved detailed summary of the last run (`--last-details`).
    RunLastDetails,
    /// Manage the task cache.
    Cache { subcmd: CacheSubcommand },
}

/// Resolved arguments for executing tasks.
///
/// Does not contain `last_details` — that case is represented by
/// [`ResolvedCommand::RunLastDetails`] instead.
#[derive(Debug)]
pub struct ResolvedRunCommand {
    /// `packageName#taskName` or `taskName`. If omitted, lists all available tasks.
    pub task_specifier: Option<TaskSpecifier>,

    pub flags: RunFlags,

    /// Additional arguments to pass to the tasks.
    pub additional_args: Vec<Str>,
}

impl RunCommand {
    /// Convert to the resolved run command, stripping the `last_details` flag.
    #[must_use]
    pub(crate) fn into_resolved(self) -> ResolvedRunCommand {
        ResolvedRunCommand {
            task_specifier: self.task_specifier,
            flags: self.flags,
            additional_args: self.additional_args,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("no task specifier provided")]
    MissingTaskSpecifier,

    #[error("--recursive and --transitive cannot be used together")]
    RecursiveTransitiveConflict,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --recursive")]
    PackageNameSpecifiedWithRecursive { package_name: Str, task_name: Str },

    #[error("--filter and --transitive cannot be used together")]
    FilterWithTransitive,

    #[error("--filter and --recursive cannot be used together")]
    FilterWithRecursive,

    #[error("cannot specify package '{package_name}' for task '{task_name}' with --filter")]
    PackageNameSpecifiedWithFilter { package_name: Str, task_name: Str },

    #[error("--filter value contains no selectors (whitespace-only)")]
    EmptyFilter,

    #[error("invalid --filter expression")]
    InvalidFilter(#[from] PackageFilterParseError),
}

impl ResolvedRunCommand {
    /// Convert to `QueryPlanRequest`, or return an error if invalid.
    ///
    /// # Errors
    ///
    /// Returns an error if conflicting flags are set or if a `--filter` expression
    /// cannot be parsed.
    pub fn into_query_plan_request(
        self,
        cwd: &Arc<AbsolutePath>,
    ) -> Result<QueryPlanRequest, CLITaskQueryError> {
        let Self {
            task_specifier,
            flags: RunFlags { recursive, transitive, ignore_depends_on, filter, .. },
            additional_args,
        } = self;

        let task_specifier = task_specifier.ok_or(CLITaskQueryError::MissingTaskSpecifier)?;

        let include_explicit_deps = !ignore_depends_on;

        let package_query = if recursive {
            if transitive {
                return Err(CLITaskQueryError::RecursiveTransitiveConflict);
            }
            if !filter.is_empty() {
                return Err(CLITaskQueryError::FilterWithRecursive);
            }
            if let Some(package_name) = task_specifier.package_name {
                return Err(CLITaskQueryError::PackageNameSpecifiedWithRecursive {
                    package_name,
                    task_name: task_specifier.task_name,
                });
            }
            PackageQuery::All
        } else if !filter.is_empty() {
            // At least one --filter was specified.
            if transitive {
                return Err(CLITaskQueryError::FilterWithTransitive);
            }
            if let Some(package_name) = task_specifier.package_name {
                return Err(CLITaskQueryError::PackageNameSpecifiedWithFilter {
                    package_name,
                    task_name: task_specifier.task_name,
                });
            }
            // Normalize: split each --filter value by whitespace into individual tokens.
            // This makes `--filter "a b"` equivalent to `--filter a --filter b` (pnpm behaviour).
            let tokens: Vec1<Str> = Vec1::try_from_vec(
                filter
                    .into_iter()
                    .flat_map(|f| f.split_ascii_whitespace().map(Str::from).collect::<Vec<_>>())
                    .collect(),
            )
            .map_err(|_| CLITaskQueryError::EmptyFilter)?;
            let parsed: Vec1<PackageFilter> = tokens.try_mapped(|f| parse_filter(&f, cwd))?;
            PackageQuery::Filters(parsed)
        } else {
            // No --filter, no --recursive: implicit cwd or package-name specifier.
            let selector = task_specifier.package_name.map_or_else(
                || PackageSelector::ContainingPackage(Arc::clone(cwd)),
                |name| PackageSelector::Name(PackageNamePattern::Exact(name)),
            );
            let traversal = if transitive {
                Some(GraphTraversal {
                    direction: TraversalDirection::Dependencies,
                    exclude_self: false,
                })
            } else {
                None
            };
            PackageQuery::Filters(Vec1::new(PackageFilter { exclude: false, selector, traversal }))
        };

        Ok(QueryPlanRequest {
            query: TaskQuery {
                package_query,
                task_name: task_specifier.task_name,
                include_explicit_deps,
            },
            plan_options: PlanOptions { extra_args: additional_args.into() },
        })
    }
}

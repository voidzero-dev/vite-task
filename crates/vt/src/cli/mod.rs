use std::sync::Arc;

use vt_graph::{TaskSpecifier, query::TaskQuery};
use vt_path::AbsolutePath;
use vt_plan::plan_request::{CacheOverride, PlanOptions, QueryPlanRequest};
use vt_str::Str;
use vt_workspace::package_filter::{PackageQueryArgs, PackageQueryError};

/// Controls how task output is displayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, usage::ValueEnum)]
pub enum LogMode {
    /// Output streams directly to the terminal as tasks produce it.
    #[default]
    Interleaved,
    /// Each line is prefixed with `[packageName#taskName]`.
    Labeled,
    /// Output is buffered per task and printed as a block after each task completes.
    Grouped,
}

#[derive(Debug, Clone, usage::Subcommands)]
pub enum CacheSubcommand {
    /// Clean up all the cache
    Clean,
}

/// Flags that control how a `run` command selects tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq, usage::Args)]
#[usage(args_override_self = false)]
#[expect(clippy::struct_excessive_bools, reason = "CLI flags are naturally boolean")]
pub struct RunFlags {
    #[usage(flatten)]
    pub package_query: PackageQueryArgs,

    /// Do not run dependencies specified in `dependsOn` fields.
    #[usage(long)]
    pub ignore_depends_on: bool,

    /// Show full detailed summary after execution.
    #[usage(short = 'v', long)]
    pub verbose: bool,

    /// Force caching on for all tasks and scripts.
    #[usage(long, conflicts = "--no-cache")]
    pub cache: bool,

    /// Force caching off for all tasks and scripts.
    #[usage(long, conflicts = "--cache")]
    pub no_cache: bool,

    /// How task output is displayed.
    #[usage(long, default = "interleaved", value_enum)]
    pub log: LogMode,

    /// Maximum number of tasks to run concurrently. Defaults to 4.
    #[usage(long)]
    pub concurrency_limit: Option<usize>,

    /// Run tasks without dependency ordering. Sets concurrency to unlimited
    /// unless `--concurrency-limit` is also specified.
    #[usage(long)]
    pub parallel: bool,
}

impl RunFlags {
    #[must_use]
    pub const fn cache_override(&self) -> CacheOverride {
        if self.cache {
            CacheOverride::ForceEnabled
        } else if self.no_cache {
            CacheOverride::ForceDisabled
        } else {
            CacheOverride::None
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Public CLI types
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Arguments for the `run` subcommand.
///
/// Contains the `--last-details` flag which is resolved into a separate
/// `ResolvedCommand::RunLastDetails` variant internally.
///
/// The automatic double-dash mode stops matching flags once the trailing
/// positional starts being filled. This means all tokens after the
/// task name are passed through to the task verbatim, preventing flags like `-v`
/// from being intercepted. Flags intended for `vp` itself (e.g. `--verbose`,
/// `-r`) must appear **before** the task name.
///
/// See <https://github.com/voidzero-dev/vite-task/issues/285>.
#[derive(Debug, Default, usage::Args)]
#[usage(args_override_self = false, about = "Run tasks", long_about = "Run tasks")]
pub struct RunCommand {
    #[usage(flatten)]
    pub(crate) flags: RunFlags,

    /// Display the detailed summary of the last run.
    #[usage(long, exclusive)]
    pub(crate) last_details: bool,

    #[usage(
        double_dash = "automatic",
        value_name = "TASK_SPECIFIER_OR_ADDITIONAL_ARG",
        long_help = "Task to run, as `packageName#taskName` or just `taskName`.\nAny arguments after the task name are forwarded to the task process.\nRunning `vp run` without a task name shows an interactive task selector."
    )]
    pub(crate) task_and_args: Vec<Str>,
}

/// Vite Task CLI subcommands.
///
/// Pass directly to `Session::main` or `HandledCommand::ViteTaskCommand`.
/// The `--last-details` flag on the `run` subcommand is resolved internally.
#[derive(Debug, usage::Subcommands)]
pub enum Command {
    /// Run tasks
    Run(RunCommand),
    /// Manage the task cache
    Cache {
        #[usage(subcommand)]
        subcmd: CacheSubcommand,
    },
}

/// The Vite Task command-line parser.
#[derive(Debug, usage::Cli)]
#[usage(
    bin = "vt",
    about = "Run tasks with Vite Task",
    long_about = "Run tasks with Vite Task",
    completion,
    unknown_flags = "error",
    args_override_self = false,
    view("vpr", root = "run")
)]
pub struct Cli {
    #[usage(subcommand)]
    pub command: Command,
}

impl Command {
    /// Resolve the parsed command into the dispatched [`ResolvedCommand`] enum.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRunCommand {
    /// `packageName#taskName` or `taskName`. If omitted, lists all available tasks.
    pub task_specifier: Option<Str>,

    pub flags: RunFlags,

    /// Additional arguments to pass to the tasks.
    pub additional_args: Vec<Str>,
}

impl RunCommand {
    /// Convert to the resolved run command, stripping the `last_details` flag.
    ///
    /// Splits `task_and_args` into `task_specifier` (the first element) and
    /// `additional_args` (everything that follows).
    #[must_use]
    pub(crate) fn into_resolved(self) -> ResolvedRunCommand {
        let mut iter = self.task_and_args.into_iter();
        let task_specifier = iter.next();
        let additional_args: Vec<Str> = iter.collect();
        ResolvedRunCommand { task_specifier, flags: self.flags, additional_args }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CLITaskQueryError {
    #[error("no task specifier provided")]
    MissingTaskSpecifier,

    #[error(transparent)]
    PackageQuery(#[from] PackageQueryError),
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
    ) -> Result<(QueryPlanRequest, bool), CLITaskQueryError> {
        let raw_specifier = self.task_specifier.ok_or(CLITaskQueryError::MissingTaskSpecifier)?;
        let task_specifier = TaskSpecifier::parse_raw(&raw_specifier);

        let cache_override = self.flags.cache_override();
        let include_explicit_deps = !self.flags.ignore_depends_on;
        let concurrency_limit = self.flags.concurrency_limit.map(|n| n.max(1));
        let parallel = self.flags.parallel;
        // Read before `into_package_query` consumes the args.
        let fail_if_no_match = self.flags.package_query.fail_if_no_match;

        let (package_query, is_cwd_only) =
            self.flags.package_query.into_package_query(task_specifier.package_name, cwd)?;

        Ok((
            QueryPlanRequest {
                query: TaskQuery {
                    package_query,
                    task_name: task_specifier.task_name,
                    include_explicit_deps,
                },
                plan_options: PlanOptions {
                    extra_args: self.additional_args.into(),
                    cache_override,
                    concurrency_limit,
                    parallel,
                    fail_if_no_match,
                },
            },
            is_cwd_only,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use usage::test::{self, Outcome, Shell};

    use super::*;

    fn parse(argv: &[&str]) -> Cli {
        let argv = test::argv(argv);
        test::parse(Cli::spec(), &argv.words(), Cli::parse_from)
            .unwrap_or_else(|error| panic!("command must parse:\n{error}"))
    }

    fn parse_run(argv: &[&str]) -> ResolvedRunCommand {
        let Command::Run(command) = parse(argv).command else { panic!("expected the run command") };
        command.into_resolved()
    }

    #[test]
    fn parses_run_options_before_the_task() {
        let command = parse_run(&[
            "run",
            "-v",
            "--cache",
            "--log",
            "grouped",
            "--concurrency-limit",
            "8",
            "--parallel",
            "build",
        ]);

        assert_eq!(command.task_specifier.as_deref(), Some("build"));
        assert!(command.additional_args.is_empty());
        assert!(command.flags.verbose);
        assert!(command.flags.cache);
        assert_eq!(command.flags.log, LogMode::Grouped);
        assert_eq!(command.flags.concurrency_limit, Some(8));
        assert!(command.flags.parallel);
    }

    #[test]
    fn forwards_all_options_after_the_task() {
        let command = parse_run(&["run", "build", "-v", "--log", "grouped", "--help"]);

        assert_eq!(command.task_specifier.as_deref(), Some("build"));
        assert_eq!(command.additional_args, ["-v", "--log", "grouped", "--help"]);
        assert!(!command.flags.verbose);
        assert_eq!(command.flags.log, LogMode::Interleaved);
    }

    #[test]
    fn forwards_values_after_an_explicit_double_dash() {
        let command = parse_run(&["run", "--", "--build", "--flag"]);

        assert_eq!(command.task_specifier.as_deref(), Some("--build"));
        assert_eq!(command.additional_args, ["--flag"]);
    }

    #[test]
    fn rejects_conflicts_duplicates_and_unknown_options() {
        for argv in [
            &["run", "--cache", "--no-cache", "build"][..],
            &["run", "--log", "grouped", "--log", "labeled", "build"],
            &["run", "--unknown", "build"],
            &["run", "--last-details", "build"],
            &["run", "--log"],
            &["run", "--log", "unknown", "build"],
        ] {
            let words = test::argv(argv);
            let outcome = test::outcome(Cli::spec(), &words.words(), Cli::parse_from);
            assert!(matches!(outcome, Outcome::Failed(_)), "{argv:?}: {outcome:?}");
        }
    }

    #[test]
    fn parses_cache_and_last_details_commands() {
        assert!(matches!(parse(&["cache", "clean"]).command, Command::Cache { .. }));

        let Command::Run(command) = parse(&["run", "--last-details"]).command else {
            panic!("expected the run command")
        };
        assert!(matches!(Command::Run(command).into_resolved(), ResolvedCommand::RunLastDetails));
    }

    #[test]
    fn returns_help_without_starting_a_process() {
        let words = test::argv(["run", "--help"]);
        let outcome = test::outcome(Cli::spec(), &words.words(), Cli::parse_from);
        let Outcome::Help(help) = outcome else { panic!("expected help, got {outcome:?}") };

        assert_eq!(help.code, 0);
        assert!(!help.stderr);
        assert!(help.text.contains("--concurrency-limit"), "{}", help.text);
        assert!(help.text.contains("TASK_SPECIFIER_OR_ADDITIONAL_ARG"), "{}", help.text);
    }

    #[test]
    fn parses_the_vpr_executable_view() {
        let cli =
            Cli::parse_from_argv(&[OsStr::new("vpr"), OsStr::new("build"), OsStr::new("--help")])
                .expect("vpr must parse as the run command");
        let Command::Run(command) = cli.command else { panic!("vpr must select run") };
        let command = command.into_resolved();

        assert_eq!(command.task_specifier.as_deref(), Some("build"));
        assert_eq!(command.additional_args, ["--help"]);
    }

    #[test]
    fn completes_commands_options_and_value_enums() {
        assert!(test::candidates(Cli::spec(), "vt r").contains(&"run".to_owned()));
        assert!(test::candidates(Cli::spec(), "vt run --l").contains(&"--log".to_owned()));
        assert_eq!(test::candidates(Cli::spec(), "vt run --log g"), ["grouped"]);
        assert!(test::candidates(Cli::spec(), "vt run build --").is_empty());

        let completion = test::completion_at(
            Cli::spec(),
            "vt run --log g ignored",
            "vt run --log g".len(),
            Shell::Bash,
        );
        assert!(completion.candidates.iter().any(|candidate| candidate.value == "grouped"));
    }

    #[test]
    fn generates_completion_scripts_for_supported_shells() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Nu, Shell::PowerShell] {
            assert!(!Cli::completion_script(shell).is_empty());
        }
        assert!(Cli::completion_script_for_alias("vpr", Shell::Bash).contains("vpr"));
    }
}

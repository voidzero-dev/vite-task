mod cache;
mod event;
mod execute;
pub(crate) mod reporter;

// Re-export types that are part of the public API
use std::{ffi::OsStr, fmt::Debug, io::IsTerminal, sync::Arc};

use cache::ExecutionCache;
pub use cache::{CacheMiss, FingerprintMismatch};
pub use event::ExecutionEvent;
use once_cell::sync::OnceCell;
pub use reporter::ExitStatus;
use reporter::LabeledReporter;
use rustc_hash::FxHashMap;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_select::SelectItem;
use vite_str::Str;
use vite_task_graph::{
    IndexedTaskGraph, TaskGraph, TaskGraphLoadError, TaskSpecifier, config::user::UserCacheConfig,
    loader::UserConfigLoader,
};
use vite_task_plan::{
    ExecutionPlan, TaskGraphLoader, TaskPlanErrorKind,
    plan_request::{PlanRequest, ScriptCommand, SyntheticPlanRequest},
    prepend_path_env,
};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::cli::{CacheSubcommand, Command, RunCommand, RunFlags};

#[derive(Debug)]
enum LazyTaskGraph<'a> {
    Uninitialized { workspace_root: WorkspaceRoot, config_loader: &'a dyn UserConfigLoader },
    Initialized(IndexedTaskGraph),
}

#[async_trait::async_trait(?Send)]
impl TaskGraphLoader for LazyTaskGraph<'_> {
    async fn load_task_graph(
        &mut self,
    ) -> Result<&vite_task_graph::IndexedTaskGraph, TaskGraphLoadError> {
        Ok(match self {
            Self::Uninitialized { workspace_root, config_loader } => {
                let graph = IndexedTaskGraph::load(workspace_root, *config_loader).await?;
                *self = Self::Initialized(graph);
                match self {
                    Self::Initialized(graph) => &*graph,
                    Self::Uninitialized { .. } => unreachable!(),
                }
            }
            Self::Initialized(graph) => &*graph,
        })
    }
}

pub struct SessionCallbacks<'a> {
    pub command_handler: &'a mut (dyn CommandHandler + 'a),
    pub user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
}

/// The result of a [`CommandHandler::handle_command`] call.
#[derive(Debug)]
pub enum HandledCommand {
    /// The command was synthesized into a task (e.g., `vp lint` → `oxlint`).
    Synthesized(SyntheticPlanRequest),
    /// The command is a vite task CLI command (e.g., `vp run build`).
    ViteTaskCommand(Command),
    /// The command should be executed verbatim as an external process.
    Verbatim,
}

/// Handles commands found in task scripts to determine how they should be executed.
///
/// The implementation should return:
/// - [`HandledCommand::Synthesized`] to replace the command with a synthetic task.
/// - [`HandledCommand::ViteTaskCommand`] when the command is a vite task CLI invocation.
/// - [`HandledCommand::Verbatim`] to execute the command as-is as an external process.
#[async_trait::async_trait(?Send)]
pub trait CommandHandler: Debug {
    /// Called for every command in task scripts to determine how it should be handled.
    async fn handle_command(
        &mut self,
        command: &mut ScriptCommand,
    ) -> anyhow::Result<HandledCommand>;
}

#[derive(derive_more::Debug)]
struct PlanRequestParser<'a> {
    command_handler: &'a mut (dyn CommandHandler + 'a),
}

#[async_trait::async_trait(?Send)]
impl vite_task_plan::PlanRequestParser for PlanRequestParser<'_> {
    async fn get_plan_request(
        &mut self,
        command: &mut ScriptCommand,
    ) -> anyhow::Result<Option<PlanRequest>> {
        match self.command_handler.handle_command(command).await? {
            HandledCommand::Synthesized(synthetic) => Ok(Some(PlanRequest::Synthetic(synthetic))),
            HandledCommand::ViteTaskCommand(cli_command) => match cli_command {
                Command::Cache { .. } => Ok(Some(PlanRequest::Synthetic(
                    command.to_synthetic_plan_request(UserCacheConfig::disabled()),
                ))),
                Command::Run(run_command) => match run_command.into_plan_request(&command.cwd) {
                    Ok(plan_request) => Ok(Some(plan_request)),
                    Err(crate::cli::CLITaskQueryError::MissingTaskSpecifier) => {
                        Ok(Some(PlanRequest::Synthetic(
                            command.to_synthetic_plan_request(UserCacheConfig::disabled()),
                        )))
                    }
                    Err(err) => Err(err.into()),
                },
            },
            HandledCommand::Verbatim => Ok(None),
        }
    }
}

/// Represents a vite task session for planning and executing tasks. A process typically has one session.
///
/// A session manages task graph loading internally and provides non-consuming methods to plan and/or execute tasks (allows multiple plans/executions per session).
pub struct Session<'a> {
    workspace_path: Arc<AbsolutePath>,
    /// A session doesn't necessarily load the task graph immediately.
    /// The task graph is loaded on-demand and cached for future use.
    lazy_task_graph: LazyTaskGraph<'a>,

    envs: Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>>,
    cwd: Arc<AbsolutePath>,

    plan_request_parser: PlanRequestParser<'a>,

    /// Cache is lazily initialized to avoid `SQLite` race conditions when multiple
    /// processes (e.g., parallel `vp lib` commands) start simultaneously.
    cache: OnceCell<ExecutionCache>,
    cache_path: AbsolutePathBuf,
}

fn get_cache_path_of_workspace(workspace_root: &AbsolutePath) -> AbsolutePathBuf {
    std::env::var("VITE_CACHE_PATH").map_or_else(
        |_| workspace_root.join("node_modules/.vite/task-cache"),
        |env_cache_path| {
            AbsolutePathBuf::new(env_cache_path.into()).expect("Cache path should be absolute")
        },
    )
}

impl<'a> Session<'a> {
    /// Initialize a session with real environment variables and cwd
    ///
    /// # Errors
    ///
    /// Returns an error if the current directory cannot be determined or
    /// if workspace initialization fails.
    pub fn init(callbacks: SessionCallbacks<'a>) -> anyhow::Result<Self> {
        let envs = std::env::vars_os()
            .map(|(k, v)| (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str())))
            .collect();
        Self::init_with(envs, vite_path::current_dir()?.into(), callbacks)
    }

    /// Ensures the task graph is loaded, loading it if necessary.
    ///
    /// # Errors
    ///
    /// Returns an error if the task graph cannot be loaded from the workspace configuration.
    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    pub async fn ensure_task_graph_loaded(
        &mut self,
    ) -> Result<&IndexedTaskGraph, TaskGraphLoadError> {
        self.lazy_task_graph.load_task_graph().await
    }

    /// Initialize a session with custom cwd, environment variables. Useful for testing.
    ///
    /// # Errors
    ///
    /// Returns an error if workspace root cannot be found or PATH env cannot be prepended.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "cwd is an Arc that gets cloned internally, pass by value is intentional"
    )]
    pub fn init_with(
        mut envs: FxHashMap<Arc<OsStr>, Arc<OsStr>>,
        cwd: Arc<AbsolutePath>,
        callbacks: SessionCallbacks<'a>,
    ) -> anyhow::Result<Self> {
        let (workspace_root, _) = find_workspace_root(&cwd)?;
        let cache_path = get_cache_path_of_workspace(&workspace_root.path);

        // Prepend workspace's node_modules/.bin to PATH
        let workspace_node_modules_bin = workspace_root.path.join("node_modules").join(".bin");
        prepend_path_env(&mut envs, &workspace_node_modules_bin)?;

        // Cache is lazily initialized on first access to avoid SQLite race conditions
        Ok(Self {
            workspace_path: Arc::clone(&workspace_root.path),
            lazy_task_graph: LazyTaskGraph::Uninitialized {
                workspace_root,
                config_loader: callbacks.user_config_loader,
            },
            envs: Arc::new(envs),
            cwd,
            plan_request_parser: PlanRequestParser { command_handler: callbacks.command_handler },
            cache: OnceCell::new(),
            cache_path,
        })
    }

    /// Primary entry point for CLI usage. Plans and executes the given command.
    ///
    /// # Errors
    ///
    /// Returns an error if planning or execution fails.
    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    #[expect(
        clippy::large_futures,
        reason = "execution plan future is large but only awaited once"
    )]
    pub async fn main(mut self, command: Command) -> anyhow::Result<ExitStatus> {
        match command {
            Command::Cache { ref subcmd } => self.handle_cache_command(subcmd),
            Command::Run(run_command) => {
                let cwd = Arc::clone(&self.cwd);
                let is_interactive =
                    std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

                // Copy flags before consuming run_command
                let flags = run_command.flags;
                let additional_args = run_command.additional_args.clone();

                match self.plan_from_cli(cwd, run_command).await {
                    Ok(plan) => {
                        let reporter =
                            LabeledReporter::new(std::io::stdout(), self.workspace_path());
                        Ok(self
                            .execute(plan, Box::new(reporter))
                            .await
                            .err()
                            .unwrap_or(ExitStatus::SUCCESS))
                    }
                    Err(err) if err.is_missing_task_specifier() => {
                        self.handle_no_task(is_interactive, None, flags, additional_args).await
                    }
                    Err(err) => {
                        if let Some(task_name) = err.task_not_found_name() {
                            let task_name = task_name.to_owned();
                            self.handle_no_task(
                                is_interactive,
                                Some(&task_name),
                                flags,
                                additional_args,
                            )
                            .await
                        } else {
                            Err(err.into())
                        }
                    }
                }
            }
        }
    }

    fn handle_cache_command(&self, subcmd: &CacheSubcommand) -> anyhow::Result<ExitStatus> {
        match subcmd {
            CacheSubcommand::Clean => {
                if self.cache_path.as_path().exists() {
                    std::fs::remove_dir_all(&self.cache_path)?;
                }
                Ok(ExitStatus::SUCCESS)
            }
        }
    }

    /// Handle the case where no task was specified or a task name was not found.
    ///
    /// In interactive mode, shows a fuzzy-searchable selection list.
    /// In non-interactive mode, prints the task list or "did you mean" suggestions.
    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    #[expect(
        clippy::large_futures,
        reason = "interactive select future is large but only awaited once"
    )]
    async fn handle_no_task(
        &mut self,
        is_interactive: bool,
        not_found_name: Option<&str>,
        flags: RunFlags,
        additional_args: Vec<Str>,
    ) -> anyhow::Result<ExitStatus> {
        let cwd = Arc::clone(&self.cwd);
        let task_graph = self.ensure_task_graph_loaded().await?;
        let mut entries = task_graph.list_tasks();
        entries.sort_unstable_by(|a, b| {
            a.task_display
                .package_name
                .cmp(&b.task_display.package_name)
                .then_with(|| a.task_display.task_name.cmp(&b.task_display.task_name))
        });

        // Find the most specific package containing the CWD (longest matching path)
        let current_package_path = entries
            .iter()
            .map(|e| &e.task_display.package_path)
            .filter(|p| cwd.as_path().starts_with(p.as_path()))
            .max_by_key(|p| p.as_path().as_os_str().len())
            .cloned();

        // Sort: current package tasks first, then others
        let (current, others): (Vec<_>, Vec<_>) = entries
            .iter()
            .partition(|e| current_package_path.as_ref() == Some(&e.task_display.package_path));

        // Build the items list: current package tasks first (unqualified name),
        // then other packages (qualified with package#task).
        let select_items: Vec<SelectItem> = current
            .iter()
            .map(|entry| SelectItem {
                label: entry.task_display.task_name.clone(),
                description: entry.command.clone(),
            })
            .chain(others.iter().map(|entry| SelectItem {
                label: vite_str::format!("{}", entry.task_display),
                description: entry.command.clone(),
            }))
            .collect();

        let header = not_found_name.map(|name| vite_str::format!("Task \"{name}\" not found."));
        let header_str = header.as_deref();

        if is_interactive {
            self.interactive_task_select(
                &select_items,
                not_found_name,
                header_str,
                flags,
                additional_args,
            )
            .await
        } else {
            Self::non_interactive_task_list(&select_items, not_found_name, header_str)
        }
    }

    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    #[expect(
        clippy::large_futures,
        reason = "execution plan future is large but only awaited once"
    )]
    async fn interactive_task_select(
        &mut self,
        items: &[SelectItem],
        not_found_name: Option<&str>,
        header: Option<&str>,
        flags: RunFlags,
        additional_args: Vec<Str>,
    ) -> anyhow::Result<ExitStatus> {
        let selection =
            vite_select::interactive_select(items, not_found_name, header, 8, |state| {
                use std::io::Write;
                let milestone_name =
                    vite_str::format!("task-select:{}:{}", state.query, state.selected_index);
                let milestone_bytes = pty_terminal_test_client::encoded_milestone(&milestone_name);
                let mut out = std::io::stdout();
                let _ = out.write_all(&milestone_bytes);
                let _ = out.flush();
            })?;

        let Some(result) = selection else {
            return Ok(ExitStatus::SUCCESS);
        };

        let selected_label = &items[result.original_index].label;

        // Parse the selected label back into a TaskSpecifier and re-run
        let task_specifier = TaskSpecifier::parse_raw(selected_label);

        let run_command =
            RunCommand { task_specifier: Some(task_specifier), flags, additional_args };

        let cwd = Arc::clone(&self.cwd);
        let plan = self.plan_from_cli(cwd, run_command).await?;
        let reporter = LabeledReporter::new(std::io::stdout(), self.workspace_path());
        Ok(self.execute(plan, Box::new(reporter)).await.err().unwrap_or(ExitStatus::SUCCESS))
    }

    fn non_interactive_task_list(
        items: &[SelectItem],
        not_found_name: Option<&str>,
        header: Option<&str>,
    ) -> anyhow::Result<ExitStatus> {
        let mut stdout = std::io::stdout().lock();

        // For the "did you mean" case, add suffix to header
        let did_you_mean_header = not_found_name
            .map(|name| vite_str::format!("Task \"{name}\" not found. Did you mean:"));
        let effective_header =
            if not_found_name.is_some() { did_you_mean_header.as_deref() } else { header };

        vite_select::print_select_list(&mut stdout, items, not_found_name, effective_header)?;

        if not_found_name.is_some() {
            // Non-interactive typo case should exit with failure
            Ok(ExitStatus::FAILURE)
        } else {
            Ok(ExitStatus::SUCCESS)
        }
    }

    /// Lazily initializes and returns the execution cache.
    /// The cache is only created when first accessed to avoid `SQLite` race conditions
    /// when multiple processes start simultaneously.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache database cannot be loaded or created.
    pub fn cache(&self) -> anyhow::Result<&ExecutionCache> {
        self.cache.get_or_try_init(|| ExecutionCache::load_from_path(&self.cache_path))
    }

    pub fn workspace_path(&self) -> Arc<AbsolutePath> {
        Arc::clone(&self.workspace_path)
    }

    pub const fn task_graph(&self) -> Option<&TaskGraph> {
        match &self.lazy_task_graph {
            LazyTaskGraph::Initialized(graph) => Some(graph.task_graph()),
            LazyTaskGraph::Uninitialized { .. } => None,
        }
    }

    pub const fn envs(&self) -> &Arc<FxHashMap<Arc<OsStr>, Arc<OsStr>>> {
        &self.envs
    }

    pub const fn cwd(&self) -> &Arc<AbsolutePath> {
        &self.cwd
    }

    /// Execute a synthetic command with cache support.
    ///
    /// This is for executing a command with cache before/without the entrypoint [`Session::main`].
    /// In vite-plus, this is used for auto-install.
    ///
    /// # Errors
    ///
    /// Returns an error if planning or execution of the synthetic command fails.
    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    #[expect(
        clippy::large_futures,
        reason = "execution plan future is large but only awaited once"
    )]
    pub async fn execute_synthetic(
        &self,
        synthetic_plan_request: SyntheticPlanRequest,
        cache_key: Arc<[Str]>,
        silent_if_cache_hit: bool,
    ) -> anyhow::Result<ExitStatus> {
        let plan = ExecutionPlan::plan_synthetic(
            &self.workspace_path,
            &self.cwd,
            synthetic_plan_request,
            cache_key,
        )?;
        let mut reporter = LabeledReporter::new(std::io::stdout(), self.workspace_path());
        reporter.set_hide_summary(true);
        reporter.set_silent_if_cache_hit(silent_if_cache_hit);
        Ok(self.execute(plan, Box::new(reporter)).await.err().unwrap_or(ExitStatus::SUCCESS))
    }

    /// Plans execution from a CLI run command.
    ///
    /// # Errors
    ///
    /// Returns an error if the plan request cannot be parsed or if planning fails.
    #[expect(
        clippy::future_not_send,
        reason = "session is single-threaded, futures do not need to be Send"
    )]
    pub async fn plan_from_cli(
        &mut self,
        cwd: Arc<AbsolutePath>,
        command: RunCommand,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        let plan_request = match command.into_plan_request(&cwd) {
            Ok(plan_request) => plan_request,
            Err(crate::cli::CLITaskQueryError::MissingTaskSpecifier) => {
                return Err(TaskPlanErrorKind::MissingTaskSpecifier.with_empty_call_stack());
            }
            Err(error) => {
                return Err(TaskPlanErrorKind::ParsePlanRequestError {
                    error: error.into(),
                    program: Str::from("vp"),
                    args: Arc::default(),
                    cwd: Arc::clone(&cwd),
                }
                .with_empty_call_stack());
            }
        };
        let plan = ExecutionPlan::plan(
            plan_request,
            &self.workspace_path,
            &cwd,
            &self.envs,
            &mut self.plan_request_parser,
            &mut self.lazy_task_graph,
        )
        .await?;
        Ok(plan)
    }
}

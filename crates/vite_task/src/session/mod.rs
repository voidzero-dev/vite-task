mod cache;
mod event;
mod execute;
pub(crate) mod reporter;

// Re-export types that are part of the public API
use std::{ffi::OsStr, fmt::Debug, sync::Arc};

use cache::ExecutionCache;
pub use cache::{CacheMiss, FingerprintMismatch};
pub use event::ExecutionEvent;
use monostate::MustBe;
use once_cell::sync::OnceCell;
pub use reporter::ExitStatus;
use reporter::LabeledReporter;
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;
use vite_task_graph::{
    IndexedTaskGraph, TaskGraph, TaskGraphLoadError,
    config::user::{UserCacheConfig, UserTaskOptions},
    loader::UserConfigLoader,
};
use vite_task_plan::{
    ExecutionPlan, TaskGraphLoader, TaskPlanErrorKind,
    plan_request::{PlanRequest, ScriptCommand, SyntheticPlanRequest},
    prepend_path_env,
};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::{
    cli::{CacheSubcommand, Command, RunCommand},
    collections::HashMap,
};

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
                    _ => unreachable!(),
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
    /// The command was synthesized into a task (e.g., `vite lint` → `oxlint`).
    Synthesized(SyntheticPlanRequest),
    /// The command is a vite-task CLI command (e.g., `vite run build`).
    ViteTaskCommand(Command),
    /// The command should be executed verbatim as an external process.
    Verbatim,
}

/// Handles commands found in task scripts to determine how they should be executed.
///
/// The implementation should return:
/// - [`HandledCommand::Synthesized`] to replace the command with a synthetic task.
/// - [`HandledCommand::ViteTaskCommand`] when the command is a vite-task CLI invocation.
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
                Command::Cache { .. } => Ok(Some(PlanRequest::Synthetic(SyntheticPlanRequest {
                    program: Arc::from(OsStr::new(command.program.as_str())),
                    args: Arc::clone(&command.args),
                    task_options: UserTaskOptions {
                        cache_config: UserCacheConfig::Disabled { cache: MustBe!(false) },
                        ..Default::default()
                    },
                    envs: Arc::clone(&command.envs),
                }))),
                Command::Run(run_command) => Ok(Some(run_command.into_plan_request(&command.cwd)?)),
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

    envs: Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
    cwd: Arc<AbsolutePath>,

    plan_request_parser: PlanRequestParser<'a>,

    /// Cache is lazily initialized to avoid SQLite race conditions when multiple
    /// processes (e.g., parallel `vite lib` commands) start simultaneously.
    cache: OnceCell<ExecutionCache>,
    cache_path: AbsolutePathBuf,
}

fn get_cache_path_of_workspace(workspace_root: &AbsolutePath) -> AbsolutePathBuf {
    if let Ok(env_cache_path) = std::env::var("VITE_CACHE_PATH") {
        AbsolutePathBuf::new(env_cache_path.into()).expect("Cache path should be absolute")
    } else {
        workspace_root.join("node_modules/.vite/task-cache")
    }
}

impl<'a> Session<'a> {
    /// Initialize a session with real environment variables and cwd
    pub fn init(callbacks: SessionCallbacks<'a>) -> anyhow::Result<Self> {
        let envs = std::env::vars_os()
            .map(|(k, v)| (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str())))
            .collect();
        Self::init_with(envs, vite_path::current_dir()?.into(), callbacks)
    }

    pub async fn ensure_task_graph_loaded(
        &mut self,
    ) -> Result<&IndexedTaskGraph, TaskGraphLoadError> {
        self.lazy_task_graph.load_task_graph().await
    }

    /// Initialize a session with custom cwd, environment variables. Useful for testing.
    pub fn init_with(
        mut envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
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
    pub async fn main(mut self, command: Command) -> anyhow::Result<ExitStatus> {
        match command {
            Command::Cache { subcmd } => self.handle_cache_command(subcmd),
            Command::Run(run_command) => {
                let cwd = Arc::clone(&self.cwd);
                let plan = self.plan_from_cli(cwd, run_command).await?;
                let reporter = LabeledReporter::new(std::io::stdout(), self.workspace_path());
                Ok(self
                    .execute(plan, Box::new(reporter))
                    .await
                    .err()
                    .unwrap_or(ExitStatus::SUCCESS))
            }
        }
    }

    fn handle_cache_command(&self, subcmd: CacheSubcommand) -> anyhow::Result<ExitStatus> {
        match subcmd {
            CacheSubcommand::Clean => {
                if self.cache_path.as_path().exists() {
                    std::fs::remove_dir_all(&self.cache_path)?;
                }
                Ok(ExitStatus::SUCCESS)
            }
        }
    }

    /// Lazily initializes and returns the execution cache.
    /// The cache is only created when first accessed to avoid SQLite race conditions
    /// when multiple processes start simultaneously.
    pub fn cache(&self) -> anyhow::Result<&ExecutionCache> {
        self.cache.get_or_try_init(|| ExecutionCache::load_from_path(self.cache_path.clone()))
    }

    pub fn workspace_path(&self) -> Arc<AbsolutePath> {
        Arc::clone(&self.workspace_path)
    }

    pub fn task_graph(&self) -> Option<&TaskGraph> {
        match &self.lazy_task_graph {
            LazyTaskGraph::Initialized(graph) => Some(graph.task_graph()),
            _ => None,
        }
    }

    pub fn envs(&self) -> &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>> {
        &self.envs
    }

    pub fn cwd(&self) -> &Arc<AbsolutePath> {
        &self.cwd
    }

    /// Execute a synthetic command with cache support.
    ///
    /// This is for executing a command with cache before/without the entrypoint [`Session::main`].
    /// In vite-plus, this is used for auto-install.
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

    pub async fn plan_from_cli(
        &mut self,
        cwd: Arc<AbsolutePath>,
        command: RunCommand,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        let plan_request = command.into_plan_request(&cwd).map_err(|error| {
            TaskPlanErrorKind::ParsePlanRequestError {
                error: error.into(),
                program: Str::from("vite"),
                args: Default::default(),
                cwd: Arc::clone(&cwd),
            }
            .with_empty_call_stack()
        })?;
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

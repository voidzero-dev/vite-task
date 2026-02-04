mod cache;
mod event;
mod execute;
pub mod reporter;

// Re-export types that are part of the public API
use std::{ffi::OsStr, fmt::Debug, sync::Arc};

use cache::ExecutionCache;
pub use cache::{CacheMiss, FingerprintMismatch};
use clap::Parser;
pub use event::ExecutionEvent;
use once_cell::sync::OnceCell;
pub use reporter::{LabeledReporter, Reporter};
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskGraph, TaskGraphLoadError, loader::UserConfigLoader};
use vite_task_plan::{
    ExecutionPlan, TaskGraphLoader, TaskPlanErrorKind,
    plan_request::{PlanRequest, SyntheticPlanRequest},
    prepend_path_env,
};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::{cli::BuiltInCommand, collections::HashMap};

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
    pub task_synthesizer: &'a mut (dyn TaskSynthesizer + 'a),
    pub user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
}

/// Handles synthesizing task plan requests from commands found in task scripts.
///
/// When a task's command references a known program (e.g., `vite lint` in a script),
/// the synthesizer converts it into a `SyntheticPlanRequest` for execution.
#[async_trait::async_trait(?Send)]
pub trait TaskSynthesizer: Debug {
    /// Called for every command in task scripts to determine if it should be synthesized.
    ///
    /// - `program` is the program name (e.g., `"vite"`).
    /// - `args` is all arguments after the program (e.g., `["lint", "--fix"]`).
    /// - `envs` is the current environment variables where the task is being planned.
    /// - `cwd` is the current working directory where the task is being planned.
    ///
    /// Returns `Ok(Some(request))` if the command is recognized and should be synthesized,
    /// `Ok(None)` if the command should be executed as a normal process.
    async fn synthesize_task(
        &mut self,
        program: &str,
        args: &[Str],
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<SyntheticPlanRequest>>;
}

#[derive(derive_more::Debug)]
struct PlanRequestParser<'a> {
    task_synthesizer: &'a mut (dyn TaskSynthesizer + 'a),
}

#[async_trait::async_trait(?Send)]
impl vite_task_plan::PlanRequestParser for PlanRequestParser<'_> {
    async fn get_plan_request(
        &mut self,
        program: &str,
        args: &[Str],
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<PlanRequest>> {
        // Try task synthesizer first (handles e.g. "vite lint" in scripts)
        if let Some(synthetic) =
            self.task_synthesizer.synthesize_task(program, args, envs, cwd).await?
        {
            return Ok(Some(PlanRequest::Synthetic(synthetic)));
        }

        // Try built-in "run" command (handles "vite run build" in scripts)
        #[derive(Parser)]
        enum BuiltInParser {
            #[clap(flatten)]
            Command(BuiltInCommand),
        }
        if let Ok(BuiltInParser::Command(built_in)) = BuiltInParser::try_parse_from(
            std::iter::once(program).chain(args.iter().map(Str::as_str)),
        ) {
            return Ok(Some(built_in.into_plan_request(cwd)?));
        }

        Ok(None)
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
            plan_request_parser: PlanRequestParser { task_synthesizer: callbacks.task_synthesizer },
            cache: OnceCell::new(),
            cache_path,
        })
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

    pub async fn plan_synthetic_task(
        &mut self,
        synthetic_plan_request: SyntheticPlanRequest,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        let plan = ExecutionPlan::plan(
            PlanRequest::Synthetic(synthetic_plan_request),
            &self.workspace_path,
            &self.cwd,
            &self.envs,
            &mut self.plan_request_parser,
            &mut self.lazy_task_graph,
        )
        .await?;
        Ok(plan)
    }

    pub async fn plan_from_cli(
        &mut self,
        cwd: Arc<AbsolutePath>,
        command: BuiltInCommand,
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

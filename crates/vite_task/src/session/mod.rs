mod cache;
mod event;
mod execute;
pub mod reporter;

// Re-export types that are part of the public API
use std::{ffi::OsStr, fmt::Debug, sync::Arc};

use cache::ExecutionCache;
pub use cache::{CacheMiss, FingerprintMismatch};
use clap::{Parser, Subcommand};
pub use event::ExecutionEvent;
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

use crate::{
    cli::{ParsedTaskCLIArgs, TaskCLIArgs},
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

pub struct SessionCallbacks<'a, CustomSubcommand> {
    pub task_synthesizer: &'a mut (dyn TaskSynthesizer<CustomSubcommand> + 'a),
    pub user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
}

#[async_trait::async_trait(?Send)]
pub trait TaskSynthesizer<CustomSubcommand>: Debug {
    fn should_synthesize_for_program(&self, program: &str) -> bool;

    /// Synthesize a synthetic task plan request for the given parsed custom subcommand.
    ///
    /// - `envs` is the current environment variables where the task is being planned.
    /// - `cwd` is the current working directory where the task is being planned.
    ///
    /// The implementor can return a different `envs` in `SyntheticPlanRequest` to customize
    /// environment variables for the synthetic task.
    async fn synthesize_task(
        &mut self,
        subcommand: CustomSubcommand,
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<SyntheticPlanRequest>;
}

#[derive(derive_more::Debug)]
#[debug(bound())] // Avoid requiring CustomSubcommand: Debug
struct PlanRequestParser<'a, CustomSubcommand> {
    task_synthesizer: &'a mut (dyn TaskSynthesizer<CustomSubcommand> + 'a),
}

impl<CustomSubcommand: clap::Subcommand> PlanRequestParser<'_, CustomSubcommand> {
    async fn get_plan_request_from_cli_args(
        &mut self,
        cli_args: ParsedTaskCLIArgs<CustomSubcommand>,
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<PlanRequest> {
        match cli_args {
            ParsedTaskCLIArgs::BuiltIn(vite_task_subcommand) => {
                Ok(vite_task_subcommand.into_plan_request(cwd)?)
            }
            ParsedTaskCLIArgs::Custom(custom_subcommand) => {
                let synthetic_plan_request =
                    self.task_synthesizer.synthesize_task(custom_subcommand, envs, cwd).await?;
                Ok(PlanRequest::Synthetic(synthetic_plan_request))
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<CustomSubcommand: clap::Subcommand> vite_task_plan::PlanRequestParser
    for PlanRequestParser<'_, CustomSubcommand>
{
    async fn get_plan_request(
        &mut self,
        program: &str,
        args: &[Str],
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<PlanRequest>> {
        Ok(
            if self.task_synthesizer.should_synthesize_for_program(program)
                && let Some(subcommand) = args.first()
                && ParsedTaskCLIArgs::<CustomSubcommand>::has_subcommand(subcommand)
            {
                let cli_args = ParsedTaskCLIArgs::<CustomSubcommand>::try_parse_from(
                    std::iter::once(program).chain(args.iter().map(Str::as_str)),
                )?;
                Some(self.get_plan_request_from_cli_args(cli_args, envs, cwd).await?)
            } else {
                None
            },
        )
    }
}

/// Represents a vite task session for planning and executing tasks. A process typically has one session.
///
/// A session manages task graph loading internally and provides non-consuming methods to plan and/or execute tasks (allows multiple plans/executions per session).
pub struct Session<'a, CustomSubcommand> {
    workspace_path: Arc<AbsolutePath>,
    /// A session doesn't necessarily load the task graph immediately.
    /// The task graph is loaded on-demand and cached for future use.
    lazy_task_graph: LazyTaskGraph<'a>,

    envs: Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
    cwd: Arc<AbsolutePath>,

    plan_request_parser: PlanRequestParser<'a, CustomSubcommand>,

    cache: ExecutionCache,
}

fn get_cache_path_of_workspace(workspace_root: &AbsolutePath) -> AbsolutePathBuf {
    if let Ok(env_cache_path) = std::env::var("VITE_CACHE_PATH") {
        AbsolutePathBuf::new(env_cache_path.into()).expect("Cache path should be absolute")
    } else {
        workspace_root.join("node_modules/.vite/task-cache")
    }
}

impl<'a, CustomSubcommand> Session<'a, CustomSubcommand> {
    /// Initialize a session with real environment variables and cwd
    pub fn init(callbacks: SessionCallbacks<'a, CustomSubcommand>) -> anyhow::Result<Self> {
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
        callbacks: SessionCallbacks<'a, CustomSubcommand>,
    ) -> anyhow::Result<Self> {
        let (workspace_root, _) = find_workspace_root(&cwd)?;
        let cache_path = get_cache_path_of_workspace(&workspace_root.path);

        // Prepend workspace's node_modules/.bin to PATH
        let workspace_node_modules_bin = workspace_root.path.join("node_modules").join(".bin");
        prepend_path_env(&mut envs, &workspace_node_modules_bin)?;

        if !cache_path.as_path().exists()
            && let Some(cache_dir) = cache_path.as_path().parent()
        {
            tracing::info!("Creating task cache directory at {}", cache_dir.display());
            std::fs::create_dir_all(cache_dir)?;
        }
        let cache = ExecutionCache::load_from_path(cache_path)?;
        Ok(Self {
            workspace_path: Arc::clone(&workspace_root.path),
            lazy_task_graph: LazyTaskGraph::Uninitialized {
                workspace_root,
                config_loader: callbacks.user_config_loader,
            },
            envs: Arc::new(envs),
            cwd,
            plan_request_parser: PlanRequestParser { task_synthesizer: callbacks.task_synthesizer },
            cache,
        })
    }

    pub fn cache(&self) -> &ExecutionCache {
        &self.cache
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
}

impl<'a, CustomSubcommand: clap::Subcommand> Session<'a, CustomSubcommand> {
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
        cli_args: TaskCLIArgs<CustomSubcommand>,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        let plan_request = self
            .plan_request_parser
            .get_plan_request_from_cli_args(cli_args.parsed, &self.envs, &cwd)
            .await
            .map_err(|error| {
                TaskPlanErrorKind::ParsePlanRequestError {
                    error,
                    program: cli_args.original[0].clone(),
                    args: cli_args.original.iter().skip(1).cloned().collect(),
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

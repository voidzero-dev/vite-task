mod cache;
mod event;
mod execute;

use std::{ffi::OsStr, fmt::Debug, sync::Arc};

use cache::TaskCache;
use clap::{Parser, Subcommand};
use vite_path::{AbsolutePath, AbsolutePathBuf};
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskGraph, TaskGraphLoadError, loader::UserConfigLoader};
use vite_task_plan::{
    ExecutionPlan, TaskGraphLoader, TaskPlanErrorKind,
    plan_request::{PlanRequest, SyntheticPlanRequest},
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

impl LazyTaskGraph<'_> {
    fn try_get(&self) -> Option<&IndexedTaskGraph> {
        match self {
            Self::Initialized(graph) => Some(graph),
            _ => None,
        }
    }
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

pub struct SessionCallbacks<'a, CustomSubCommand> {
    pub task_synthesizer: &'a mut (dyn TaskSynthesizer<CustomSubCommand> + 'a),
    pub user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
}

#[async_trait::async_trait(?Send)]
pub trait TaskSynthesizer<CustomSubCommand>: Debug {
    fn should_synthesize_for_program(&self, program: &str) -> bool;
    async fn synthesize_task(
        &mut self,
        subcommand: CustomSubCommand,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<SyntheticPlanRequest>;
}

#[derive(derive_more::Debug)]
#[debug(bound())] // Avoid requiring CustomSubCommand: Debug
struct PlanRequestParser<'a, CustomSubCommand> {
    task_synthesizer: &'a mut (dyn TaskSynthesizer<CustomSubCommand> + 'a),
}

impl<CustomSubCommand: clap::Subcommand> PlanRequestParser<'_, CustomSubCommand> {
    async fn get_plan_request_from_cli_args(
        &mut self,
        cli_args: ParsedTaskCLIArgs<CustomSubCommand>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<PlanRequest> {
        match cli_args {
            ParsedTaskCLIArgs::BuiltIn(vite_task_subcommand) => {
                Ok(vite_task_subcommand.into_plan_request(cwd)?)
            }
            ParsedTaskCLIArgs::Custom(custom_subcommand) => {
                let synthetic_plan_request =
                    self.task_synthesizer.synthesize_task(custom_subcommand, cwd).await?;
                Ok(PlanRequest::Synthetic(synthetic_plan_request))
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<CustomSubCommand: clap::Subcommand> vite_task_plan::PlanRequestParser
    for PlanRequestParser<'_, CustomSubCommand>
{
    async fn get_plan_request(
        &mut self,
        program: &str,
        args: &[Str],
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<PlanRequest>> {
        Ok(
            if self.task_synthesizer.should_synthesize_for_program(program)
                && let Some(subcommand) = args.first()
                && ParsedTaskCLIArgs::<CustomSubCommand>::has_subcommand(subcommand)
            {
                let cli_args = ParsedTaskCLIArgs::<CustomSubCommand>::try_parse_from(
                    std::iter::once(program).chain(args.iter().map(Str::as_str)),
                )?;
                Some(self.get_plan_request_from_cli_args(cli_args, cwd).await?)
            } else {
                None
            },
        )
    }
}

/// Represents a vite task session for planning and executing tasks. A process typically has one session.
///
/// A session manages task graph loading internally and provides non-consuming methods to plan and/or execute tasks (allows multiple plans/executions per session).
pub struct Session<'a, CustomSubCommand> {
    workspace_path: Arc<AbsolutePath>,
    /// A session doesn't necessarily load the task graph immediately.
    /// The task graph is loaded on-demand and cached for future use.
    lazy_task_graph: LazyTaskGraph<'a>,

    envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
    cwd: Arc<AbsolutePath>,

    plan_request_parser: PlanRequestParser<'a, CustomSubCommand>,

    cache: TaskCache,
}

fn get_cache_path_of_workspace(workspace_root: &AbsolutePath) -> AbsolutePathBuf {
    if let Ok(env_cache_path) = std::env::var("VITE_CACHE_PATH") {
        AbsolutePathBuf::new(env_cache_path.into()).expect("Cache path should be absolute")
    } else {
        workspace_root.join("node_modules/.vite/task-cache")
    }
}

impl<'a, CustomSubCommand> Session<'a, CustomSubCommand> {
    /// Initialize a session with real environment variables and cwd
    pub fn init(callbacks: SessionCallbacks<'a, CustomSubCommand>) -> anyhow::Result<Self> {
        let envs = std::env::vars_os()
            .map(|(k, v)| (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str())))
            .collect();
        Self::init_with(envs, vite_path::current_dir()?.into(), callbacks)
    }

    /// Initialize a session with custom cwd, environment variables. Useful for testing.
    pub fn init_with(
        envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
        cwd: Arc<AbsolutePath>,
        callbacks: SessionCallbacks<'a, CustomSubCommand>,
    ) -> anyhow::Result<Self> {
        let (workspace_root, _) = find_workspace_root(&cwd)?;
        let cache_path = get_cache_path_of_workspace(&workspace_root.path);

        if !cache_path.as_path().exists()
            && let Some(cache_dir) = cache_path.as_path().parent()
        {
            tracing::info!("Creating task cache directory at {}", cache_dir.display());
            std::fs::create_dir_all(cache_dir)?;
        }
        let cache = TaskCache::load_from_path(cache_path)?;
        Ok(Self {
            workspace_path: Arc::clone(&workspace_root.path),
            lazy_task_graph: LazyTaskGraph::Uninitialized {
                workspace_root,
                config_loader: callbacks.user_config_loader,
            },
            envs,
            cwd,
            plan_request_parser: PlanRequestParser { task_synthesizer: callbacks.task_synthesizer },
            cache,
        })
    }

    pub fn cache(&self) -> &TaskCache {
        &self.cache
    }

    pub fn task_graph(&self) -> Option<&TaskGraph> {
        match &self.lazy_task_graph {
            LazyTaskGraph::Initialized(graph) => Some(graph.task_graph()),
            _ => None,
        }
    }
}

/// Represents a planned execution of tasks in a session, including information for caching.
#[derive(Debug)]
pub struct SessionExecutionPlan {
    /// The original command-line arguments used to create this execution plan, excluding the program name.
    ///
    /// It's used to create cache keys for direct executions. See `DirectExecutionCacheKey` for details.
    original_args: Arc<[Str]>,

    /// The current working directory used to create this execution plan.
    ///
    /// It's used to create cache keys for direct executions. See `DirectExecutionCacheKey` for details.
    cwd: Arc<AbsolutePath>,

    /// The actual content of the execution plan.
    plan: vite_task_plan::ExecutionPlan,
}

impl<'a, CustomSubCommand: clap::Subcommand> Session<'a, CustomSubCommand> {
    pub async fn plan(
        &mut self,
        cwd: Arc<AbsolutePath>,
        cli_args: TaskCLIArgs<CustomSubCommand>,
    ) -> Result<SessionExecutionPlan, vite_task_plan::Error> {
        let plan_request = self
            .plan_request_parser
            .get_plan_request_from_cli_args(cli_args.parsed, &cwd)
            .await
            .map_err(|error| {
                TaskPlanErrorKind::ParsePlanRequestError { error }.with_empty_call_stack()
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
        Ok(SessionExecutionPlan {
            original_args: cli_args.original.iter().skip(1).cloned().collect(), // Skip program name
            cwd,
            plan,
        })
    }
}

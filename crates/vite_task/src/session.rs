use std::{ffi::OsStr, fmt::Debug, sync::Arc};

use clap::Parser;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{IndexedTaskGraph, TaskGraph, TaskGraphLoadError, loader::UserConfigLoader};
use vite_task_plan::{
    ExecutionPlan, TaskGraphLoader, TaskPlanErrorKind,
    plan_request::{PlanRequest, SyntheticPlanRequest},
};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::{CLIArgs, collections::HashMap};

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

pub struct SessionCallbacks<'a, CustomSubCommand> {
    task_synthesizer: &'a mut (dyn TaskSynthesizer<CustomSubCommand> + 'a),
    user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
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
        cli_args: CLIArgs<CustomSubCommand>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<PlanRequest> {
        match cli_args {
            CLIArgs::ViteTaskSubCommand(vite_task_subcommand) => {
                Ok(vite_task_subcommand.into_plan_request(cwd)?)
            }
            CLIArgs::Custom(custom_subcommand) => {
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
        if !self.task_synthesizer.should_synthesize_for_program(program) {
            return Ok(None);
        }
        let cli_args = CLIArgs::<CustomSubCommand>::try_parse_from(
            std::iter::once(program).chain(args.iter().map(Str::as_str)),
        )?;
        Ok(Some(self.get_plan_request_from_cli_args(cli_args, cwd).await?))
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
        Ok(Self {
            workspace_path: Arc::clone(&workspace_root.path),
            lazy_task_graph: LazyTaskGraph::Uninitialized {
                workspace_root,
                config_loader: callbacks.user_config_loader,
            },
            envs,
            cwd,
            plan_request_parser: PlanRequestParser { task_synthesizer: callbacks.task_synthesizer },
        })
    }

    pub fn task_graph(&self) -> Option<&TaskGraph> {
        match &self.lazy_task_graph {
            LazyTaskGraph::Initialized(graph) => Some(graph.task_graph()),
            _ => None,
        }
    }
}

impl<'a, CustomSubCommand: clap::Subcommand> Session<'a, CustomSubCommand> {
    pub async fn plan(
        &mut self,
        cli_args: CLIArgs<CustomSubCommand>,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        let plan_request = self
            .plan_request_parser
            .get_plan_request_from_cli_args(cli_args, &self.cwd)
            .await
            .map_err(|error| {
                TaskPlanErrorKind::ParsePlanRequestError { error }.with_empty_call_stack()
            })?;
        ExecutionPlan::plan(
            plan_request,
            &self.workspace_path,
            &self.cwd,
            &self.envs,
            &mut self.plan_request_parser,
            &mut self.lazy_task_graph,
        )
        .await
    }
}

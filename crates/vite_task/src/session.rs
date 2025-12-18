use std::{ffi::OsStr, sync::Arc};

use vite_path::AbsolutePath;
use vite_task_graph::{IndexedTaskGraph, TaskGraphLoadError, loader::UserConfigLoader};
use vite_task_plan::{
    ExecutionPlan, PlanRequestParser, TaskGraphLoader, plan_request::PlanRequest,
};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::collections::HashMap;

#[derive(Debug)]
enum LazyTaskGraph<'a> {
    Uninitialized { workspace_root: WorkspaceRoot, config_loader: &'a dyn UserConfigLoader },
    Initialized(IndexedTaskGraph),
}

#[async_trait::async_trait]
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
    plan_request_parser: &'a mut (dyn PlanRequestParser + 'a),
    user_config_loader: &'a mut (dyn UserConfigLoader + 'a),
}

pub struct Session<'a> {
    workspace_path: Arc<AbsolutePath>,
    /// A session doesn't necessarily load the task graph immediately.
    /// The task graph is loaded on-demand and cached for future use.
    lazy_task_graph: LazyTaskGraph<'a>,

    envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
    cwd: Arc<AbsolutePath>,

    plan_request_parser: &'a mut (dyn PlanRequestParser + 'a),
}

impl<'a> Session<'a> {
    /// Initialize a session with real environment variables and cwd
    pub fn init(callbacks: SessionCallbacks<'a>) -> anyhow::Result<Self> {
        let envs = std::env::vars_os()
            .map(|(k, v)| (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str())))
            .collect();
        Self::init_with(envs, vite_path::current_dir()?.into(), callbacks)
    }

    /// Initialize a session with custom cwd, environment variables. Useful for testing.
    pub fn init_with(
        envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
        cwd: Arc<AbsolutePath>,
        callbacks: SessionCallbacks<'a>,
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
            plan_request_parser: callbacks.plan_request_parser,
        })
    }

    pub async fn plan(
        &mut self,
        plan_request: PlanRequest,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        ExecutionPlan::plan(
            plan_request,
            &self.workspace_path,
            &self.cwd,
            &self.envs,
            self.plan_request_parser,
            &mut self.lazy_task_graph,
        )
        .await
    }
}

use std::{ffi::OsStr, sync::Arc};

use futures_core::future::BoxFuture;
use vite_path::AbsolutePath;
use vite_task_graph::{IndexedTaskGraph, TaskGraphLoadError, loader::UserConfigLoader};
use vite_task_plan::{ExecutionPlan, plan_request::PlanRequest};
use vite_workspace::{WorkspaceRoot, find_workspace_root};

use crate::collections::HashMap;

#[derive(Debug)]
enum LazyTaskGraph {
    Uninitialized(WorkspaceRoot),
    Initialized(Arc<IndexedTaskGraph>),
}
impl LazyTaskGraph {
    async fn get(
        &mut self,
        config_loader: &(impl UserConfigLoader + '_),
    ) -> Result<Arc<IndexedTaskGraph>, TaskGraphLoadError> {
        Ok(match self {
            Self::Uninitialized(workspace_root) => {
                let graph = IndexedTaskGraph::load(workspace_root, config_loader).await?;
                let graph = Arc::new(graph);
                *self = Self::Initialized(Arc::clone(&graph));
                graph
            }
            Self::Initialized(graph) => Arc::clone(&graph),
        })
    }
}

pub trait SessionCallbacks: UserConfigLoader {
    /// Get a plan request for the given program and args in the given cwd.
    ///
    /// - If it returns `Ok(None)`, the command will be spawned as a normal process.
    /// - If it returns `Ok(Some(PlanRequest::Query)`, the command will be expanded as a `ExecutionPlan` with a task graph queried from the returned `TaskQuery`.
    /// - If it returns `Ok(Some(PlanRequest::Synthetic)`, the command will become a `ExecutionPlan` with the synthetic task.
    fn get_plan_request(
        &self,
        program: &str,
        args: &[vite_str::Str],
        cwd: &AbsolutePath,
    ) -> BoxFuture<'_, anyhow::Result<Option<PlanRequest>>>;
}

pub struct Session {
    workspace_path: Arc<AbsolutePath>,
    /// A session doesn't necessarily load the task graph immediately.
    /// The task graph is loaded on-demand and cached for future use.
    lazy_task_graph: LazyTaskGraph,

    envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
    cwd: Arc<AbsolutePath>,
}

impl Session {
    /// Initialize a session with real environment variables and cwd
    pub fn init() -> anyhow::Result<Self> {
        let envs = std::env::vars_os()
            .map(|(k, v)| (Arc::<OsStr>::from(k.as_os_str()), Arc::<OsStr>::from(v.as_os_str())))
            .collect();
        Self::init_with(envs, vite_path::current_dir()?.into())
    }

    /// Initialize a session with custom cwd, environment variables. Useful for testing.
    pub fn init_with(
        envs: HashMap<Arc<OsStr>, Arc<OsStr>>,
        cwd: Arc<AbsolutePath>,
    ) -> anyhow::Result<Self> {
        let (workspace_root, _) = find_workspace_root(&cwd)?;
        Ok(Self {
            workspace_path: Arc::clone(&workspace_root.path),
            lazy_task_graph: LazyTaskGraph::Uninitialized(workspace_root),
            envs,
            cwd,
        })
    }

    pub async fn plan(
        &mut self,
        plan_request: PlanRequest,
    ) -> Result<ExecutionPlan, vite_task_plan::Error> {
        // ExecutionPlan::plan(plan_request, &self.workspace_path, &self.cwd, &self.envs, todo!())
        //     .await
        todo!()
    }
}

#[derive(Debug)]
struct PlanCallbacks<'a> {
    lazy_task_graph: &'a mut LazyTaskGraph,
}
impl<'a> vite_task_plan::PlanRequestParser for PlanCallbacks<'a> {
    // fn load_task_graph(
    //     &mut self,
    //     cwd: &AbsolutePath,
    // ) -> BoxFuture<'_, Result<Arc<vite_task_graph::IndexedTaskGraph>, TaskGraphLoadError>> {
    //     Box::pin(async move {
    //         let config_loader = vite_task_graph::loader::JsonUserConfigLoader::new(cwd);
    //         self.lazy_task_graph.get(&config_loader).await
    //     })
    // }

    fn get_plan_request(
        &self,
        program: &str,
        args: &[vite_str::Str],
        cwd: &AbsolutePath,
    ) -> BoxFuture<'_, anyhow::Result<Option<PlanRequest>>> {
        todo!()
    }
}

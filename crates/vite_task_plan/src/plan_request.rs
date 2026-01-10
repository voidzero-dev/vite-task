use std::{collections::HashMap, ffi::OsStr, sync::Arc};

use vite_str::Str;
use vite_task_graph::{config::user::UserTaskOptions, query::TaskQuery};

#[derive(Debug)]
pub struct PlanOptions {
    pub extra_args: Arc<[Str]>,
}

#[derive(Debug)]
pub struct QueryPlanRequest {
    /// The query to run against the task graph. For example: `-r build`
    pub query: TaskQuery,

    /// Other options affecting the planning process, not the task graph querying itself.
    ///
    /// For example: `-- arg1 arg2`
    pub plan_options: PlanOptions,
}

/// The request to run a synthetic task, like `vite lint` or `vite exec ...`
/// Synthetic tasks are not defined in the task graph, but are generated on-the-fly.
#[derive(Debug)]
pub struct SyntheticPlanRequest {
    /// The program to execute
    pub program: Arc<OsStr>,

    /// The arguments to pass to the program
    pub args: Arc<[Str]>,

    /// The task options as if it's defined in `vite.config.*`
    pub task_options: UserTaskOptions,

    /// The cache key for execution directly issued from user command line.
    /// It typically includes the subcommand name and all args after it. (e.g. `["lint", "--fix"]` for `vite lint --fix`)
    pub direct_execution_cache_key: Arc<[Str]>,

    /// All environment variables to set for the synthetic task.
    ///
    /// This is set in the plan stage before resolving envs for caching.
    /// Therefore, these envs are subject to env configurations in `UserTaskOptions`.
    ///
    /// - To set envs that are not subject to caching but still passed to the spawned child, use `task_options` to configure `pass_through_envs`.
    /// - To set envs that should be fingerprinted, use `task_options` to configure `envs`.
    /// - If neither is set, and caching is enabled, these envs will have not effect.
    pub envs: Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
}

#[derive(Debug)]
pub enum PlanRequest {
    /// The request to run tasks queried from the task graph, like `vite run ...` or `vite run-many ...`.
    Query(QueryPlanRequest),
    /// The request to run a synthetic task (not defined in the task graph), like `vite lint` or `vite exec ...`.
    Synthetic(SyntheticPlanRequest),
}

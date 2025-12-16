use std::sync::Arc;

use vite_str::Str;
use vite_task_graph::{config::UserTaskConfig, query::TaskQuery};

use crate::SpawnCommandKind;

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
    /// The command to execute in the synthetic task.
    pub command_kind: SpawnCommandKind,

    pub user_config: UserTaskConfig,
}

#[derive(Debug)]
pub enum PlanRequest {
    /// The request to run tasks queried from the task graph, like `vite run ...` or `vite run-many ...`.
    Query(QueryPlanRequest),
    Synthetic(SyntheticPlanRequest),
}

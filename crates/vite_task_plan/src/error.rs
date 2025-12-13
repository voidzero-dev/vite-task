use std::sync::Arc;

use vite_path::AbsolutePath;
use vite_str::Str;

/// Errors that can occur when planning a specific execution from a task .
#[derive(Debug, thiserror::Error)]
pub enum TaskPlanError {
    #[error("Failed to parse command '{subcommand}'")]
    CallbackParseArgsError {
        subcommand: Str,
        #[source]
        error: anyhow::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to load task graph")]
    TaskGraphLoadError(
        #[source]
        #[from]
        vite_task_graph::TaskGraphLoadError,
    ),

    #[error("Failed to query tasks from task graph")]
    TaskQueryError(
        #[source]
        #[from]
        vite_task_graph::query::TaskQueryError,
    ),

    #[error("Failed to plan execution for task '{package_name}#{task_name}'")]
    TaskPlanError {
        package_name: Str,
        task_name: Str,
        #[source]
        task_plan_error: TaskPlanError,
    },
}

use std::env::JoinPathsError;

use crate::{
    context::{PlanContext, TaskCallStackDisplay, TaskRecursionError},
    envs::ResolveEnvError,
};

#[derive(Debug, thiserror::Error)]
pub enum CdCommandError {
    #[error("No home directory found for 'cd' command with no arguments")]
    NoHomeDirectory,

    #[error("Too many args for 'cd' command")]
    ToManyArgs,
}

/// Errors that can occur when planning a specific execution from a task .
#[derive(Debug, thiserror::Error)]
pub enum TaskPlanErrorKind {
    #[error("Failed to load task graph")]
    TaskGraphLoadError(
        #[source]
        #[from]
        vite_task_graph::TaskGraphLoadError,
    ),

    #[error("Failed to execute 'cd' command")]
    CdCommandError(
        #[source]
        #[from]
        CdCommandError,
    ),

    #[error("Failed to query tasks from task graph")]
    TaskQueryError(
        #[source]
        #[from]
        vite_task_graph::query::TaskQueryError,
    ),

    #[error(transparent)]
    TaskRecursionDetected(#[from] TaskRecursionError),

    #[error("Invalid vite task command")]
    ParsePlanRequestError {
        #[source]
        error: anyhow::Error,
    },

    #[error("Failed to add node_modules/.bin to PATH environment variable")]
    AddNodeModulesBinPathError {
        #[source]
        join_paths_error: JoinPathsError,
    },

    #[error("Failed to resolve environment variables")]
    ResolveEnvError(#[source] ResolveEnvError),
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to plan execution, task call stack: {task_call_stack}")]
pub struct Error {
    task_call_stack: TaskCallStackDisplay,

    #[source]
    kind: TaskPlanErrorKind,
}

impl TaskPlanErrorKind {
    pub fn with_empty_call_stack(self) -> Error {
        Error { task_call_stack: TaskCallStackDisplay::default(), kind: self }
    }
}

pub(crate) trait TaskPlanErrorKindResultExt {
    type Ok;
    /// Attach the current task call stack from the planning context to the error.
    fn with_plan_context(self, context: &PlanContext<'_>) -> Result<Self::Ok, Error>;

    /// Attach an empty task call stack to the error.
    fn with_empty_call_stack(self) -> Result<Self::Ok, Error>;
}

impl<T> TaskPlanErrorKindResultExt for Result<T, TaskPlanErrorKind> {
    type Ok = T;

    /// Attach the current task call stack from the planning context to the error.
    fn with_plan_context(self, context: &PlanContext<'_>) -> Result<T, Error> {
        match self {
            Ok(value) => Ok(value),
            Err(kind) => {
                let task_call_stack = context.display_call_stack();
                Err(Error { task_call_stack, kind })
            }
        }
    }

    fn with_empty_call_stack(self) -> Result<T, Error> {
        match self {
            Ok(value) => Ok(value),
            Err(kind) => Err(kind.with_empty_call_stack()),
        }
    }
}

use std::env::JoinPathsError;

use vite_task_graph::display::TaskDispay;

use crate::{
    context::{PlanContext, TaskCallStackDisplay, TaskCycleError},
    envs::ResolveEnvError,
};

/// Errors that can occur when planning a specific execution from a task .
#[derive(Debug, thiserror::Error)]
pub enum TaskPlanErrorKind {
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

    #[error(transparent)]
    TaskCycleDetected(#[from] TaskCycleError),

    #[error("Invalid vite task command")]
    ParsePlanRequestError {
        #[source]
        error: anyhow::Error,
    },

    #[error("Failed to add node_modules/.bin to PATH environment variable")]
    AddNodeModulesBinPathError {
        /// This error occurred before parse the command of the task,
        /// so the task call stack doesn't contain the current task (no command_span yet).
        /// This field is where the error occurred, while the task call stack is the stack leading to it.s
        task_display: TaskDispay,
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

pub(crate) trait TaskPlanErrorKindResultExt {
    type Ok;
    /// Attach the current task call stack from the planning context to the error.
    fn with_task_call_stack(self, context: &PlanContext<'_>) -> Result<Self::Ok, Error>;
}

impl<T> TaskPlanErrorKindResultExt for Result<T, TaskPlanErrorKind> {
    type Ok = T;

    /// Attach the current task call stack from the planning context to the error.
    fn with_task_call_stack(self, context: &PlanContext<'_>) -> Result<T, Error> {
        match self {
            Ok(value) => Ok(value),
            Err(kind) => {
                let task_call_stack = context.display_call_stack();
                Err(Error { task_call_stack, kind })
            }
        }
    }
}

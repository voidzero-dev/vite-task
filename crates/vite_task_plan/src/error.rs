use std::{env::JoinPathsError, sync::Arc};

use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task_graph::{TaskNodeIndex, display::TaskDispay};

use crate::context::{
    PlanContext, TaskCallStackDisplay, TaskCallStackFrameDisplay, TaskCycleError,
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

    #[error("Failed to parse command")]
    CallbackParseArgsError {
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
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to plan execution, task call stack: {task_call_stack}")]
pub struct Error {
    task_call_stack: TaskCallStackDisplay,

    #[source]
    kind: TaskPlanErrorKind,
}

pub trait TaskPlanErrorKindResultExt {
    type Ok;
    fn with_task_call_stack(self, context: &PlanContext<'_>) -> Result<Self::Ok, Error>;
}

impl<T> TaskPlanErrorKindResultExt for Result<T, TaskPlanErrorKind> {
    type Ok = T;

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

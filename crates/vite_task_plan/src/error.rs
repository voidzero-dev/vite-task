#[expect(
    clippy::disallowed_types,
    reason = "Arc<Path> is used for non-UTF-8 path data in error types"
)]
use std::path::Path;
use std::{env::JoinPathsError, ffi::OsStr, sync::Arc};

use vite_path::{AbsolutePath, relative::InvalidPathDataError};
use vite_str::Str;
use vite_task_graph::display::TaskDisplay;

use crate::{context::TaskRecursionError, envs::ResolveEnvError};

#[derive(Debug, thiserror::Error)]
pub enum CdCommandError {
    #[error("No home directory found for 'cd' command with no arguments")]
    NoHomeDirectory,

    #[error("Too many args for 'cd' command")]
    ToManyArgs,
}

#[derive(Debug, thiserror::Error)]
pub struct WhichError {
    pub program: Arc<OsStr>,
    pub path_env: Option<Arc<OsStr>>,
    pub cwd: Arc<AbsolutePath>,
    #[source]
    pub error: which::Error,
}
impl std::fmt::Display for WhichError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Failed to find executable {} under cwd {} with ",
            self.program.display(),
            self.cwd.as_path().display()
        )?;
        if let Some(path_env) = &self.path_env {
            write!(f, "PATH: {}", path_env.display())?;
        } else {
            write!(f, "No PATH")?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathFingerprintErrorKind {
    #[error("Path {path:?} is outside of the workspace {workspace_path:?}")]
    PathOutsideWorkspace { path: Arc<AbsolutePath>, workspace_path: Arc<AbsolutePath> },
    #[error("Path {path:?} contains characters that make it non-portable")]
    NonPortableRelativePath {
        #[expect(clippy::disallowed_types, reason = "path may contain non-UTF-8 data")]
        path: Arc<Path>,
        #[source]
        error: InvalidPathDataError,
    },
}

#[derive(Debug)]
pub enum PathType {
    Cwd,
    Program,
    PackagePath,
}
impl std::fmt::Display for PathType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cwd => write!(f, "current working directory"),
            Self::Program => write!(f, "program path"),
            Self::PackagePath => write!(f, "package path"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to fingerprint {path_type}")]
pub struct PathFingerprintError {
    pub path_type: PathType,
    #[source]
    pub kind: PathFingerprintErrorKind,
}

/// Errors that can occur when planning a specific execution from a task.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to plan tasks from `{command}` in task {task_display}")]
    NestPlan {
        task_display: TaskDisplay,
        command: Str,
        #[source]
        error: Box<Self>,
    },

    #[error("Failed to load task graph")]
    TaskGraphLoad(
        #[source]
        #[from]
        vite_task_graph::TaskGraphLoadError,
    ),

    #[error("Failed to execute 'cd' command")]
    CdCommand(
        #[source]
        #[from]
        CdCommandError,
    ),

    #[error(transparent)]
    ProgramNotFound(#[from] WhichError),

    #[error(transparent)]
    PathFingerprint(#[from] PathFingerprintError),

    #[error("Failed to query tasks from task graph")]
    TaskQuery(
        #[source]
        #[from]
        vite_task_graph::query::TaskQueryError,
    ),

    #[error(transparent)]
    TaskRecursionDetected(#[from] TaskRecursionError),

    #[error("Invalid vite task command: {program} with args {args:?} under cwd {cwd:?}")]
    ParsePlanRequest {
        program: Str,
        args: Arc<[Str]>,
        cwd: Arc<AbsolutePath>,
        #[source]
        error: anyhow::Error,
    },

    #[error("Failed to add node_modules/.bin to PATH environment variable")]
    AddNodeModulesBinPath {
        #[source]
        join_paths_error: JoinPathsError,
    },

    #[error("Failed to resolve environment variables")]
    ResolveEnv(#[source] ResolveEnvError),

    #[error("No task specifier provided for 'run' command")]
    MissingTaskSpecifier,
}

impl Error {
    #[must_use]
    pub const fn is_missing_task_specifier(&self) -> bool {
        matches!(self, Self::MissingTaskSpecifier)
    }

    /// If this error represents a top-level task-not-found lookup failure,
    /// returns the task name that the user typed.
    ///
    /// Returns `None` if the error occurred in a nested task (wrapped in `NestPlan`),
    /// since nested task errors should propagate as-is rather than triggering
    /// interactive task selection.
    #[must_use]
    pub fn task_not_found_name(&self) -> Option<&str> {
        match self {
            Self::TaskQuery(vite_task_graph::query::TaskQueryError::SpecifierLookupError {
                specifier,
                ..
            }) => Some(specifier.task_name.as_str()),
            _ => None,
        }
    }
}

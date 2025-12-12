use std::sync::Arc;

use vite_path::AbsolutePath;
use vite_str::Str;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to parse command '{subcommand}' in package at {package_path:?}")]
    CallbackParseArgsError {
        package_path: Arc<AbsolutePath>,
        subcommand: Str,
        #[source]
        error: anyhow::Error,
    },

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
}

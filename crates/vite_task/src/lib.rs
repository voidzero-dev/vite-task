mod cache;
mod cli;
mod collections;
mod config;
mod error;
mod execute;
mod fingerprint;
mod fs;
mod maybe_str;
mod schedule;
mod session;
mod types;
mod ui;

// Public exports for vite-plus-cli to use
pub use cache::TaskCache;
pub use cli::CLIArgs;
pub use config::{ResolvedTask, Workspace};
pub use error::Error;
pub use execute::{CURRENT_EXECUTION_ID, EXECUTION_SUMMARY_DIR};
pub use schedule::{ExecutionPlan, ExecutionStatus, ExecutionSummary};
pub use session::{Session, SessionCallbacks, TaskSynthesizer};
pub use types::ResolveCommandResult;
pub use vite_task_graph::loader;
pub use vite_task_plan::plan_request;

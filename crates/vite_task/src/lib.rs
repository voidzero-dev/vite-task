mod cli;
mod collections;
mod maybe_str;
pub mod session;

// Public exports for vite_task_bin
pub use cli::BuiltInCommand;
pub use session::{LabeledReporter, Reporter, Session, SessionCallbacks, TaskSynthesizer};
pub use vite_task_graph::{
    config::{
        self,
        user::{EnabledCacheConfig, UserCacheConfig, UserTaskConfig, UserTaskOptions},
    },
    loader,
};
/// get_path_env is useful for TaskSynthesizer implementations. Re-export it here.
pub use vite_task_plan::get_path_env;
pub use vite_task_plan::plan_request;

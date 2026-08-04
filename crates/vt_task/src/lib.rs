mod cli;
mod collections;
mod napi_client;
pub mod session;

// Public exports for vt_task_bin
pub use cli::{CacheSubcommand, Command, RunCommand, RunFlags};
pub use session::{
    CommandHandler, ExitStatus, HandledCommand, Session, SessionConfig, print_error,
};
pub use vt_task_graph::{
    config::{
        self,
        user::{EnabledCacheConfig, UserCacheConfig, UserTaskConfig, UserTaskOptions},
    },
    loader,
};
/// Re-exports useful for `CommandHandler` implementations.
pub use vt_task_plan::get_path_env;
pub use vt_task_plan::{MARKER_ENV_NAME, plan_request, plan_request::ScriptCommand};

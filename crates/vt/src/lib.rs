#[expect(
    clippy::allow_attributes,
    reason = "usage-rs derive output does not inherit item-level lint attributes"
)]
#[allow(
    clippy::disallowed_types,
    clippy::pub_underscore_fields,
    reason = "usage-rs generates parser state with String and underscore-prefixed fields"
)]
mod cli;
mod collections;
mod napi_client;
pub mod session;

// Public exports for vt_bin
pub use cli::{
    CacheSubcommand, Cli, Command, CompletionData, CompletionItem, LogMode, RunCommand, RunFlags,
    complete, completion_request, completion_uses_workspace_data,
};
pub use session::{
    CommandHandler, ExitStatus, HandledCommand, Session, SessionConfig, print_error,
};
pub use vt_graph::{
    config::{
        self,
        user::{EnabledCacheConfig, UserCacheConfig, UserTaskConfig, UserTaskOptions},
    },
    loader,
};
/// Re-exports useful for `CommandHandler` implementations.
pub use vt_plan::get_path_env;
pub use vt_plan::{MARKER_ENV_NAME, plan_request, plan_request::ScriptCommand};

use native_str::NativeStr;
use rustc_hash::FxHashMap;
use wincode::{SchemaRead, SchemaWrite};

pub const IPC_ENV_NAME: &str = "VP_RUN_IPC_NAME";

/// Path to the Node client module that JS/TS tools `require()` to talk to
/// the runner.
///
/// Implementation-detail leakage (`napi`, `.node`, `addon`) is intentionally
/// kept out of the name: from the consumer's point of view this is just a
/// path they can `require()`. The `NODE_` scope reserves room for a future
/// C-ABI client library advertised via its own env var for non-Node
/// consumers.
pub const NODE_CLIENT_PATH_ENV_NAME: &str = "VP_RUN_NODE_CLIENT_PATH";

/// IPC request frame sent by tools to the runner.
///
/// `DisableCache` is fire-and-forget: the runner processes it when it
/// arrives and never writes a response. `GetEnv` and `GetEnvs` are
/// round-trips and pair with the matching response types below.
///
/// Fire-and-forget is safe because nothing in the runner observes individual
/// IPC events live — the recorded set is only consumed *after* the per-task
/// IPC driver has drained the connection, which happens after the child
/// process exits. So a tool can `flush + exit` and the server's drain phase
/// will still consume every buffered frame.
#[derive(Debug, SchemaWrite, SchemaRead)]
pub enum Request<'a> {
    GetEnv { name: &'a NativeStr, tracked: bool },
    GetEnvs { pattern: &'a str, tracked: bool },
    DisableCache,
}

#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct GetEnvResponse {
    pub env_value: Option<Box<NativeStr>>,
}

#[derive(Debug, SchemaWrite, SchemaRead)]
pub struct GetEnvsResponse {
    /// Match snapshot for the glob pattern. Keys/values are byte-faithful
    /// (`NativeStr`) so non-UTF-8 env values are preserved over the wire.
    pub entries: FxHashMap<Box<NativeStr>, Box<NativeStr>>,
}

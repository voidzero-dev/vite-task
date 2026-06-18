//! Node addon that exposes a `load()` factory which returns a
//! `RunnerClient` JS class instance bound to the runner's IPC connection.
//! Not intended to be published directly — the runner hands the compiled
//! `.node` file to child processes via the `VP_RUN_NODE_CLIENT_PATH` env
//! var, and the JS wrapper in `@voidzero-dev/vite-task-client`
//! `require()`s it lazily.
//!
//! The factory shape (`load() -> RunnerClient`, rather than methods
//! exported at the top level) is a deliberate layer of indirection so
//! the addon can evolve over time: a future wrapper can pass an options
//! argument (e.g. a version field) and receive a differently-shaped
//! addon, without breaking older addons that ignore the argument.
//!
//! `load()` is callable only inside a runner-spawned task: when the IPC
//! env is absent or the connection refuses, `load()` throws and the JS
//! wrapper falls into no-op mode.

// The napi boundary forces std `String` through function signatures; clippy's
// blanket bans on disallowed types / needless-pass-by-value / missing Errors
// sections are all about pure-Rust call sites and don't apply here (JS never
// reads rustdoc). `disallowed_macros` is allowed because `napi-derive` expands
// to `std::format!` inside `check_status!`, and the macro output isn't ours
// to rewrite.
#![expect(
    clippy::disallowed_macros,
    clippy::disallowed_types,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    reason = "napi bindings require owned std String + std::format! at the JS boundary"
)]
use std::{collections::HashMap, ffi::OsStr};

use napi::{Either, Error, Result};
use napi_derive::napi;
use vite_task_client::{Client, GetEnvsQuery};

/// Options for [`RunnerClient::get_env`] and [`RunnerClient::get_envs`].
///
/// Modeled as a JS plain object rather than a positional boolean so future
/// knobs (e.g. a `default` value) can be added without an ABI break on the
/// JS wrapper side.
///
/// Every field is optional so the napi addon — the cross-version API
/// stability boundary between the runner-shipped `.node` and the
/// separately-npm-published JS wrapper — can fill in defaults and let old
/// wrappers keep working against new runners (and vice versa).
#[napi(object)]
pub struct GetEnvOptions {
    /// Whether the runner should record this env as a cache-key dependency.
    /// Defaults to `true`.
    pub tracked: Option<bool>,
}

#[napi(object)]
pub struct GetEnvsPrefixQuery {
    pub prefix: String,
}

/// Handle returned by [`load`]. Holds the IPC connection and exposes the
/// runner-side operations as instance methods.
#[napi]
pub struct RunnerClient {
    client: Client,
}

#[napi]
impl RunnerClient {
    #[napi]
    pub fn ignore_input(&self, path: String) -> Result<()> {
        self.client
            .ignore_input(OsStr::new(&path))
            .map_err(|err| err_string(vite_str::format!("{err}")))
    }

    #[napi]
    pub fn ignore_output(&self, path: String) -> Result<()> {
        self.client
            .ignore_output(OsStr::new(&path))
            .map_err(|err| err_string(vite_str::format!("{err}")))
    }

    #[napi]
    pub fn disable_cache(&self) -> Result<()> {
        self.client.disable_cache().map_err(|err| err_string(vite_str::format!("{err}")))
    }

    #[napi]
    pub fn get_env(&self, name: String, options: Option<GetEnvOptions>) -> Result<Option<String>> {
        let tracked = options.and_then(|o| o.tracked).unwrap_or(true);
        let value = self
            .client
            .get_env(OsStr::new(&name), tracked)
            .map_err(|err| err_string(vite_str::format!("{err}")))?;
        value.map_or(Ok(None), |value| {
            value.to_str().map(|s| Some(s.to_owned())).ok_or_else(|| {
                err_string(vite_str::format!("env value for {name} is not valid UTF-8"))
            })
        })
    }

    #[napi]
    pub fn get_envs(
        &self,
        query: Either<String, GetEnvsPrefixQuery>,
        options: Option<GetEnvOptions>,
    ) -> Result<HashMap<String, String>> {
        let tracked = options.and_then(|o| o.tracked).unwrap_or(true);
        let matches = match &query {
            Either::A(pattern) => {
                self.client.get_envs(GetEnvsQuery::Glob(pattern.as_str()), tracked)
            }
            Either::B(prefix) => {
                self.client.get_envs(GetEnvsQuery::Prefix(&prefix.prefix), tracked)
            }
        }
        .map_err(|err| err_string(vite_str::format!("{err}")))?;
        let mut result = HashMap::with_capacity(matches.len());
        for (name, value) in matches {
            let name = name.to_str().ok_or_else(|| {
                err_static("env name matched by getEnvs query is not valid UTF-8")
            })?;
            let value = value.to_str().ok_or_else(|| {
                err_string(vite_str::format!("env value for {name} is not valid UTF-8"))
            })?;
            result.insert(name.to_owned(), value.to_owned());
        }
        Ok(result)
    }
}

/// Connect to the runner and return a [`RunnerClient`]. Throws when the
/// IPC env is missing or the connection fails.
#[napi]
pub fn load() -> Result<RunnerClient> {
    #[expect(
        clippy::disallowed_methods,
        reason = "client bootstrap reads the live process env to find runner IPC handoff"
    )]
    let client = Client::from_envs(std::env::vars_os())
        .map_err(|err| {
            err_string(vite_str::format!("vp run client: failed to connect to runner IPC: {err}"))
        })?
        .ok_or_else(|| {
            err_static(
                "vp run client: runner IPC env is not set; this module is only usable \
                 inside a `vp run` task",
            )
        })?;
    Ok(RunnerClient { client })
}

fn err_static(msg: &'static str) -> Error {
    Error::from_reason(msg)
}

fn err_string(msg: vite_str::Str) -> Error {
    Error::from_reason(msg.as_str())
}

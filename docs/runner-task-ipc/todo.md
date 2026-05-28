# TODO

## Native addon should return `null` on connect failure

**Status**: not started.

**Decision**: when `Client::from_envs` returns `Ok(None)` (env missing) or `Err` (connect failed), the `.node` addon's `require()` must resolve to `null` instead of throwing.

### Why

- **Impossible to misuse** — a caller writes `const addon = require(path); if (!addon) return;` once. You can't call a method on `null`, so there's no silent-no-op trap.
- **Smallest API surface** — one null-check at load time replaces per-call `try/catch` or per-call readiness checks everywhere.
- **Reserves room to upgrade** — we can later promote truly unexpected errors (bugs, misconfiguration) to throw while keeping the expected "no runner / no socket" cases as `null`, without changing tools or the JS wrapper. Committing to "throw on failure" today closes that door forever.

### Why not the alternatives

- **Throw (today)** — forces every third-party consumer into `try/catch` forever. Forgetting it crashes the tool in non-runner contexts.
- **Readiness flag on an always-returned object** — pushes the no-op decision into the methods. Every call site either needs a guard or relies on silent no-op (the tool thinks it's reporting, but isn't).

### How

napi-rs 3.x has no hook to change what its `napi_register_module_v1` returns ([napi-3.8.5/src/bindgen_runtime/module_register.rs:487-545](https://) hard-codes `Ok(exports)`), so we have to bypass it:

- Enable napi's `noop` feature on [crates/vite_task_client_napi/Cargo.toml](../../crates/vite_task_client_napi/Cargo.toml) — disables napi-rs's own `napi_register_module_v1` and avoids a duplicate-symbol linker error.
- Drop `napi-derive` and the `#[napi]` decorators. Rewrite [crates/vite_task_client_napi/src/lib.rs](../../crates/vite_task_client_napi/src/lib.rs) using raw `napi::sys::*`: one `#[unsafe(no_mangle)] extern "C" fn napi_register_module_v1`, four `extern "C"` callbacks, properties registered via `sys::napi_define_properties`.
- On `Ok(Some(client))`: store client in a `thread_local! { static CLIENT: OnceCell<Client> }` and populate `exports`. On any other outcome: return `sys::napi_get_null(env)`.
- Update [packages/vite-task-client/index.js](../../packages/vite-task-client/index.js) — drop the `try/catch` in `load()`; `require()`'s result is already `null` or an object.
- Add an e2e test that spawns Node without `VP_RUN_IPC_NAME` and asserts `require(addonPath) === null`.

Estimated size: ~200 lines. We only depend on `sys::*`, which is the stable Node ABI, so napi-rs internal churn can't break us.

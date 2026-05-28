# Server API & Lifecycle

## Goal

The IPC server runs per spawn execution **only when fspy is enabled**, letting tools report runtime-only facts to the runner (`ignoreInputs`, `ignoreOutputs`, `disableCache`, `getEnv`). The runner uses these reports alongside fspy's tracked accesses for cache correctness.

## Key principles

1. **Server doesn't take a cancellation token.** The caller signals "stop accepting" via `StopAccepting::signal()`. The server has no awareness of external cancellation.
2. **Handler is moved in, returned out.** The caller doesn't keep a reference. The driver owns the handler; on drain completion it returns it by value. No self-reference, no `&H` lifetime.
3. **`CancellationToken` is internal** — hidden from the public API (exposed only via `StopAccepting`).
4. **Driver is `!Send`**, lifetime bounded by `H`'s lifetime — if `H: 'static`, the driver is `'static`; if `H` borrows, the driver respects that.

## Server API

```rust
pub trait Handler {
    fn ignore_input(&self, path: &Arc<AbsolutePath>);
    fn ignore_output(&self, path: &Arc<AbsolutePath>);
    fn disable_cache(&self);
    fn get_env(&self, name: &str, tracked: bool) -> Option<Arc<OsStr>>;
}

pub fn serve<'h, H: Handler + 'h>(
    handler: H,
) -> io::Result<(impl Iterator<Item = (&'static OsStr, OsString)>, ServerHandle<'h, H>)>;

pub struct ServerHandle<'h, H> {
    pub driver: LocalBoxFuture<'h, H>,
    pub stop_accepting: StopAccepting,
}

pub struct StopAccepting { /* opaque */ }
impl StopAccepting {
    pub fn signal(self);
}
```

## Driver semantics

The driver future, when polled:

1. **Accept phase** — accepts new clients and pumps per-client futures (`FuturesUnordered`) until `StopAccepting::signal()` fires.
2. **Listener teardown** — drops listener; Unix socket file auto-cleaned via `tempfile::NamedTempFile`.
3. **Drain phase** — waits for in-flight per-client futures to complete naturally (each ends on client EOF).
4. **Returns `H`** — the owned handler that was moved in at `serve()`.

Dropping the driver before it resolves tears everything down immediately. Handler is dropped without being returned.

## Lifecycle in `execute_spawn`

### When to start

Only when fspy is enabled (`cache_metadata.input_config.includes_auto`). No fspy → no IPC server.

### Construction (at `ExecutionMode` build time)

`serve()` yields an env-pair iterator that the caller chains directly into the spawn's envs. The specific env var(s) used for IPC handoff are an implementation detail between the server and client crates — the runner never has to know their names.

```rust
let (ipc_envs, server) = serve(IpcRecorder::new(env_config))?;
let envs = cmd.all_envs.iter().map(|(k, v)| (&**k, &**v)).chain(ipc_envs);
let child = spawn(&cmd, envs, true, SpawnStdio::Piped, token).await?;
// After the child is spawned, nothing else needs the IPC envs.

let fspy = FspyState {
    negatives,
    server,  // ServerHandle<'h, IpcRecorder>
};
```

### `FspyState` shape

```rust
struct FspyState {
    negatives: Vec<wax::Glob<'static>>,
    server: ServerHandle<IpcRecorder>,
}
```

**Not stored:**

- IPC env name/value — consumed once to build the spawn envs, dropped immediately.
- `handler` — lives inside `server.driver`'s async state; recovered by value when the driver resolves.

### Driving the server during `pipe_stdio` / `child.wait`

The driver is polled as an extra arm in the existing `tokio::select!` blocks. `LocalBoxFuture<'static, H>` is `Unpin`, so `&mut driver` is a valid select arm:

```rust
tokio::select! {
    r = &mut pipe_fut => r,
    _recorder = &mut fspy_state.server.driver => {
        unreachable!("driver resolved before stop_accepting.signal()")
    }
}
```

The driver only resolves after `stop_accepting.signal()` + drain — neither happens during these phases, so the branch is unreachable.

### Completion paths

```rust
// Normal exit:
if !fast_fail_token.is_cancelled() && !interrupt_token.is_cancelled() {
    if let Some(fspy_state) = fspy.take() {
        fspy_state.server.stop_accepting.signal();
        let recorder = fspy_state.server.driver.await;
        // recorder.into_reports() flows into cache-update
    }
}

// Cancellation: fspy dropped at scope end → driver dropped → teardown.
```

## Design-decision log

### Why no `'static` bound on `H`?

The driver future _owns_ the handler (via `Rc<H>` internally). It doesn't need to outlive `H` — it just needs `H` to outlive the future. So the signature is `serve<'h, H: Handler + 'h>` and the returned `ServerHandle<'h, H>` carries the lifetime. If the caller's `H` is `'static`, the driver is `'static`; if `H` borrows, the driver respects that.

If the caller wants to store `ServerHandle` in a struct without a lifetime parameter, they can use a `'static` handler (naturally satisfied by handlers that own all their state via `RefCell<...>` + cloned data).

### Handler is owned by the driver, not shared via `Rc`

The driver's async function owns `handler: H` as a local. Per-client futures borrow `&handler` from that same async state; Rust's async-fn state machine makes this self-borrow sound (the state is pinned and never moves). All per-client futures live inside `FuturesUnordered` which is also part of the same state — borrow scopes are contained.

When drain completes and all per-client futures have been dropped, the outer async returns `handler` by move. No `Rc`, no `try_unwrap`, no panic possible.

### Why return `H` from the driver?

Caller doesn't keep the handler around separately. Avoids `Rc::try_unwrap` at the call site. Makes it impossible to forget recovering the state.

### Why `StopAccepting::signal(self)` instead of exposing `CancellationToken`?

- Hides the implementation (could swap `CancellationToken` for `oneshot` or `Notify` later).
- Reads as intent: "stop accepting" vs. "cancel".
- `self`-consuming method signals one-shot semantics.
- Keeps the public API free of `tokio_util` types.

### Why not pass `shutdown: impl Future` or `CancellationToken`?

Earlier direction: "server doesn't care about cancellation token; it simply stops accepting when the process exits." The caller doesn't have a token to pass — they have a moment (child exit) when they want to stop accepting. `StopAccepting::signal()` is that moment.

### On `spawn()` changes (deferred)

`spawn()` will need to accept extra envs (e.g. `envs: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>`) so the caller can inject the IPC envs without cloning `Arc<BTreeMap>`. Not part of this step.

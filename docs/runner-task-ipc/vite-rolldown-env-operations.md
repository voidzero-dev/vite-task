# Vite & Rolldown Environment Variable Operations

## Vite

### Reads that affect build output (must be tracked for cache correctness)

| File                                                                                                                             | Line | Variable                       | Effect on output                                  |
| -------------------------------------------------------------------------------------------------------------------------------- | ---- | ------------------------------ | ------------------------------------------------- |
| [config.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/config.ts#L1383)                                   | 1383 | `NODE_ENV`                     | Build mode, affects dead-code elimination         |
| [config.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/config.ts#L1661)                                   | 1661 | `VITE_USER_NODE_ENV`           | User-set NODE_ENV from `.env` file                |
| [build.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/build.ts#L644)                                      | 644  | `NODE_ENV`                     | Preserved for bundler                             |
| [plugins/clientInjections.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/clientInjections.ts#L52) | 52   | `NODE_ENV`                     | Injected into client bundle                       |
| [plugins/define.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/plugins/define.ts#L20)                     | 20   | `NODE_ENV`                     | Define replacement in output                      |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L1281)                 | 1281 | `NODE_ENV`                     | Dep pre-bundling config                           |
| [env.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/env.ts#L86)                                           | 86   | `VITE_*` (all matching prefix) | Injected into client bundle via `import.meta.env` |

### Reads that do not affect output (untracked)

| File                                                                                                             | Line | Variable                | Effect                         |
| ---------------------------------------------------------------------------------------------------------------- | ---- | ----------------------- | ------------------------------ |
| [logger.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/logger.ts#L84)                     | 84   | `CI`                    | Disables color output only     |
| [build.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/build.ts#L1067)                     | 1067 | `CI`                    | Disables TTY progress only     |
| [utils.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/utils.ts#L176)                      | 176  | `DEBUG`                 | Debug logging only             |
| [optimizer/index.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/optimizer/index.ts#L1269) | 1269 | `npm_config_user_agent` | Package manager detection only |

### Writes to `process.env`

| File                                                                                           | Line | Variable             | Reason                                                |
| ---------------------------------------------------------------------------------------------- | ---- | -------------------- | ----------------------------------------------------- |
| [config.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/config.ts#L1389) | 1389 | `NODE_ENV`           | Sets default if unset                                 |
| [config.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/config.ts#L1664) | 1664 | `NODE_ENV`           | Overrides to `'development'` if user set it in `.env` |
| [env.ts](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/env.ts#L62)         | 62   | `VITE_USER_NODE_ENV` | Stores NODE_ENV read from `.env`                      |

### `.env` file loading

Handled by `loadEnv()` at [env.ts:27](https://github.com/vitejs/vite/blob/v8.0.8/packages/vite/src/node/env.ts#L27). Reads `.env`, `.env.local`, `.env.{mode}`, `.env.{mode}.local` from `envDir`. All `VITE_*` vars become `import.meta.env.*` in the client bundle.

This is **file input fingerprinting**, not an env var concern — fspy automatically tracks the `readFileSync` calls on `.env` files as inferred inputs.

---

## Rolldown

### Rust — reads

| File                                                                                                                         | Line | Variable                        | Effect                      |
| ---------------------------------------------------------------------------------------------------------------------------- | ---- | ------------------------------- | --------------------------- |
| [rolldown_binding/src/lib.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown_binding/src/lib.rs#L88) | 88   | `ROLLDOWN_MAX_BLOCKING_THREADS` | Tokio blocking thread count |
| [rolldown_binding/src/lib.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown_binding/src/lib.rs#L95) | 95   | `ROLLDOWN_WORKER_THREADS`       | Tokio worker thread count   |
| [rolldown_tracing/src/lib.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown_tracing/src/lib.rs#L25) | 25   | `RD_LOG`                        | Tracing log levels          |
| [rolldown_tracing/src/lib.rs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/crates/rolldown_tracing/src/lib.rs#L33) | 33   | `RD_LOG_OUTPUT`                 | Log output mode             |

None of these affect build output.

### JS (NAPI binding loader) — reads

| File                                                                                                                              | Line | Variable                        | Effect                                             |
| --------------------------------------------------------------------------------------------------------------------------------- | ---- | ------------------------------- | -------------------------------------------------- |
| [packages/rolldown/src/binding.cjs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/packages/rolldown/src/binding.cjs#L64) | 64   | `NAPI_RS_NATIVE_LIBRARY_PATH`   | Custom native lib path for loading `.node` binding |
| [packages/rolldown/src/binding.cjs](https://github.com/rolldown/rolldown/blob/v1.0.0-rc.15/packages/rolldown/src/binding.cjs#L80) | 80   | `NAPI_RS_ENFORCE_VERSION_CHECK` | Version mismatch behavior                          |

---

## Implications for `getEnv` IPC

Today, env vars read from `process.env` inside the task process are invisible to
the runner — no file read happens, so fspy cannot track them. The runner's current
`env`/`untrackedEnv` config requires the user to declare them manually.

With `getEnv` IPC, a Vite plugin could request vars at runtime and have them
automatically fingerprinted:

```ts
buildStart() {
  await getEnv('NODE_ENV', { tracked: true })   // affects output — fingerprint it
  await getEnv('CI', { tracked: false })         // affects behavior only — pass through
}
```

The key vars for Vite cache correctness via `getEnv`:

- **`NODE_ENV`** — affects dead-code elimination, define replacements, and `import.meta.env.MODE`
- **`VITE_*`** — any matching var is injected into the client bundle; all must be tracked

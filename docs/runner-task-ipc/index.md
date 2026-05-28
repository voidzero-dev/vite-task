# runner-aware tools

## Motivation

Report information from the tools to the runner, to help runner cache results without needing user's manual configs.

### What information vite-task knows without runner-awareness of tools?

- All files that are read/written by the tools
- All directory that are read/written by the tools

### What information vite-task doesn't know without runner-awareness of tools?

- **Why** did the tool read/write the file/directory? (e.g. files in cache should not be considered as inputs even when they are read by the tool, and should not be considered as outputs even when they are written by the tool)
- **What** env variables are the tool interested in? (they are not available to the tool's process env if the user doesn't explicitly define them in `env` in the config)
- **Whether** the tool needs be cached at all? (e.g. dev server doesn't need to be cached, but build does)

## Implementation

Workflow:

1. For each spawn execution, `vite_task` starts an IPC server via `vite_task_server::serve` and passes the server's connection info plus the path to a materialized node addon into the child's env.
2. The task process loads the addon through `@voidzero-dev/vite-task-client` and reports back over IPC: which reads/writes to ignore, which envs it needs (returned by the runner from the spawn's resolved env map), and whether to disable caching.
3. When the task exits, the server drains and hands its collected reports back to `vite_task`, which feeds them into the cache layer.

Crate / package responsibilities:

- `vite_task_ipc_shared` — wincode-encoded request/response types and the env-var names used to hand connection info to children.
- `vite_task_server` — `Handler` trait, `serve` entry point, and a built-in `Recorder` handler that stores what the cache layer consumes.
- `vite_task_client` — sync blocking Rust client; `Client::from_envs` no-ops when the IPC env is absent.
- `vite_task_client_napi` — NAPI cdylib exposing the client's methods to Node.
- `materialized_artifact` — shared machinery for embedding the cdylib into `vp` and writing it to a temp file on first use (extracted from `fspy`).
- `@voidzero-dev/vite-task-client` — npm package with JSDoc-typed wrappers (`ignoreInput`, `ignoreOutput`, `disableCache`, `getEnv`, `getEnvs`). Lazy-loads the addon and silently no-ops when it can't connect, so tools don't break when run outside `vp`.

Notes:

- ignored input/output files reported from the runner are considered as part of `{ auto: true }`, which means if the user defines `input`/`output` without `auto: true`, in the config, the runner will only consider the files defined in the config as inputs/outputs, and ignore what's reported from the tools.
- envs requested from the tools are additional to the envs defined in the config. User config always wins: if an env is already defined in the config (e.g. as `untrackedEnv`), the tool cannot override it (e.g. upgrade it to tracked).

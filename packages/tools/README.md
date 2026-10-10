This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. The command runs TypeScript directly using the Node version in `.node-version`.

```sh
remote-cache-server start
VP_REMOTE_CACHE=read-write remote-cache-server run vt run build
remote-cache-server stop
```

An E2E case starts its own backend in its first step and stops it in its last, so the backend keeps its state for the whole case. Each subcommand works in the current directory, the case's directory:

- `remote-cache-server start` starts the backend in the background on free loopback ports and returns once it's ready. The backend runs in its own session and doesn't use the terminal, so the step can finish and Ctrl-C in later steps doesn't reach it. It writes its endpoint, `http://127.0.0.1:<port>/projects/test`, to `remote-cache/server.json`, and its output to `remote-cache/server.log`.
- `remote-cache-server run COMMAND [ARGS...]` runs the command with `VP_REMOTE_CACHE_URL` set to the endpoint. The fixed base path gives the endpoint a namespace path. The command inherits stdio and handles Ctrl-C, which `run` ignores. `run` exits with the command's exit code.
- `remote-cache-server corrupt-blob NUMBER` overwrites a stored blob, numbered as in the request lines, with other bytes.
- `remote-cache-server stop` stops the backend. It fails if the backend answered a request with a 5xx status or couldn't be reached.

A backend whose case directory disappears stops by itself, as does one that has had no requests for ten minutes, e.g. because its case timed out before the stop step.

The endpoint is a tap in front of the backend. It forwards every request and response unchanged and records a line for each response, in the order of the responses. Each line has the method, the path below the base path, and the status. Successful fetch responses add their kind. Blob IDs are random, so blob paths show the blob's number in upload order instead, keeping snapshots deterministic. After the command exits, `run` prints the lines for the requests it caused to stderr, and `stop` prints any that are left:

```text
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

The backend keeps its state in `remote-cache/`. `state.json` holds the entries and associations, with keys and values hex-encoded. Each blob is a file in `remote-cache/blobs/` named by its blob ID, a random UUID.

The backend implements `POST /fetch`, `POST /store`, and `GET /blob/{blob_id}` from the [remote cache server API](https://github.com/voidzero-dev/vite-task/pull/713). A fetch that matches neither key gets a `404` with the plain-text body `Not found`. Keys, values, and blobs are opaque bytes without length limits. There is no authentication.

Run `pnpm --filter vite-task-tools check` for type checking. Run `cargo test -p vt_bin --test e2e_snapshots -- remote_cache --ignored` for the snapshots that use the backend.

Those snapshots are skipped on Windows because the PTY launcher cannot execute pnpm command shims.

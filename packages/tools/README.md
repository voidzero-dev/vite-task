This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. The command runs TypeScript directly using the Node version in `.node-version`.

```sh
VP_REMOTE_CACHE=read-write remote-cache-server vt run build
```

`remote-cache-server COMMAND [ARGS...]` starts the backend on a free loopback port and runs the command with `VP_REMOTE_CACHE_URL` set to the endpoint, `http://127.0.0.1:<port>/projects/test`. The fixed base path gives every endpoint a namespace path. The wrapper takes no options and passes all arguments to the command unchanged. The command inherits stdio and handles Ctrl-C, which the wrapper ignores. When it exits, the server stops and the wrapper exits with the command's exit code.

After the command exits, the wrapper prints one line to stderr for each request it served, in the order of the responses. Each line has the method, the path below the base path, and the status. Successful fetch responses add their kind:

```text
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

State persists in `remote-cache/` in the current directory, so consecutive commands share it. Each E2E case has its own directory and state. `state.json` holds the entries, associations, and next blob ID, with keys and values hex-encoded. Each blob is a file in `remote-cache/blobs/` named by its blob ID. Blob IDs are sequential strings and continue across invocations, keeping snapshots deterministic.

The backend implements `POST /fetch`, `POST /store`, and `GET /blob/{blob_id}` from the [remote cache server API](https://github.com/voidzero-dev/vite-task/pull/713). A fetch that matches neither key gets a `404` with the plain-text body `Not found`. Keys, values, and blobs are opaque bytes without length limits. There is no authentication.

Run `pnpm --filter vite-task-tools check` for type checking. Run `cargo test -p vt_bin --test e2e_snapshots -- remote_cache --ignored` for the snapshots that use the backend.

Those snapshots are skipped on Windows because the PTY launcher cannot execute pnpm command shims.

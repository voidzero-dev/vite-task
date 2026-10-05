This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. The command runs TypeScript directly using the Node version in `.node-version`.

```sh
remote-cache-server start
VP_REMOTE_CACHE=read-write remote-cache-server run --github-actions vt run build
remote-cache-server stop
```

An E2E case starts its own backend in its first step and stops it in its last, so the backend keeps its state for the whole case. Each subcommand works in the current directory, the case's directory:

- `remote-cache-server start` starts the backend in the background on free loopback ports and returns once it's ready. The backend runs in its own session and doesn't use the terminal, so the step can finish and Ctrl-C in later steps doesn't reach it. It writes its endpoint, `http://127.0.0.1:<port>/projects/test`, to `remote-cache/server.json`, and its output to `remote-cache/server.log`.
- `remote-cache-server run [--github-actions] COMMAND [ARGS...]` runs the command with `VP_REMOTE_CACHE_URL` set to the endpoint. The fixed base path gives the endpoint a namespace path. With `--github-actions`, the command runs as if in a GitHub Actions job with `id-token: write`: `ACTIONS_ID_TOKEN_REQUEST_URL` points to the backend's stand-in for GitHub's token service, and `ACTIONS_ID_TOKEN_REQUEST_TOKEN` defaults to `main-push`. The command inherits stdio and handles Ctrl-C, which `run` ignores. `run` exits with the command's exit code.
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

The backend is [Vite+'s Cloudflare remote cache](https://github.com/voidzero-dev/vite-plus-remote-cache-cloudflare), at the commit that `package.json` pins, in workerd through Miniflare, with the compatibility settings, bindings, and limits in its `wrangler.jsonc`, and its namespaces limited to `test`. Like a deployment, its database has every migration applied, and the namespace is registered for the endpoint and the repository `owner/repository`. Responses follow [its protocol](https://github.com/voidzero-dev/vite-plus-remote-cache-cloudflare#protocol): for example, a fetch that matches no entry gets a `404`.

It only accepts a store with a GitHub Actions token for a push to the main branch of the registered repository, whose audience is the endpoint. It answers other stores with `401` and `Invalid credentials` for a missing or invalid token, and `403` and `Write not permitted` for a token that the write policy doesn't allow. The Worker fetches GitHub's key set to verify tokens and gets the stand-in's key instead; any other outbound request fails. The stand-in signs tokens for the workflow run that the request token stands for:

| Request token  | Workflow run                              |
| -------------- | ----------------------------------------- |
| `main-push`    | A push to `main` of `owner/repository`    |
| `pull-request` | A pull request against `owner/repository` |

The Worker keeps its D1 database and R2 bucket in `remote-cache/state/`.

Run `pnpm --filter vite-task-tools check` for type checking. Run `cargo test -p vt_bin --test e2e_snapshots -- remote_cache --ignored` for the snapshots that use the backend.

Those snapshots are skipped on Windows because the PTY launcher cannot execute pnpm command shims, and on musl because workerd's prebuilt binaries require glibc.

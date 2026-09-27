This package provides Node.js dependencies and commands for the test suites.

## Remote cache backend

Run `pnpm install` at the repository root to install `remote-cache-server` into this package's `node_modules/.bin`. The workspace self-dependency makes pnpm link the package's own commands there. The E2E harness already includes that directory in PATH. The command runs TypeScript directly using the Node version in `.node-version`.

```sh
VP_REMOTE_CACHE=read-write remote-cache-server vt run build
```

`remote-cache-server COMMAND [ARGS...]` starts the [public cache service](../remote-cache/README.md) in workerd through Miniflare, serves it on a free loopback port, and runs the command with `VP_REMOTE_CACHE_URL` set to the endpoint, `http://127.0.0.1:<port>/projects/test`. The wrapper takes no options and passes all arguments to the command unchanged. The command inherits stdio. When it exits, the server stops and the wrapper exits with the command's exit code.

The service accepts uploads only with a GitHub Actions token. The wrapper signs one for each store request with a key it generates, and the service fetches that key in place of GitHub's. Everything else reaches the service unchanged, so responses follow [its protocol](../remote-cache/README.md#protocol): for example, a fetch that matches no entry is a `404`.

After the command exits, the wrapper prints one line to stderr for each request it served, in the order of the responses. Each line has the method, the path below the endpoint, and the status. Successful fetches add their kind:

```text
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

State persists in `remote-cache/` in the current directory, so consecutive commands share it. Each E2E case has its own directory and state. The service keeps its D1 database and R2 bucket in `remote-cache/state/`.

Blob IDs are random, so the wrapper numbers blobs in upload order, and request lines show the number in place of the ID. The numbers continue across invocations, keeping snapshots deterministic, and `remote-cache/blobs.json` lists the IDs in order. When the wrapper exits, it copies each blob to `remote-cache/blobs/<number>`. If a copy has changed by the next invocation, the wrapper stores its contents as the blob, so a test can corrupt an archive by overwriting the file.

Run `pnpm --filter vite-task-tools check` for type checking. Run `cargo test -p vt_bin --test e2e_snapshots -- remote_cache --ignored` for the snapshots that use the backend.

Those snapshots are skipped on Windows because the PTY launcher cannot execute pnpm command shims, and on musl because workerd's prebuilt binaries require glibc.

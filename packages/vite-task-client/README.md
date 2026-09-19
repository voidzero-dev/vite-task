# @voidzero-dev/vite-task-client

A tiny Node.js client that lets your tool talk to the
[Vite+](https://github.com/voidzero-dev/vite-plus) task runner (`vp run`)
it's running under. Use it to hand the runner more precise
cache-correctness information than it can infer from the outside.

Outside a runner-managed task, every call is a graceful no-op — you can
call into this from a tool that's also used standalone without any
runtime detection or conditionals.

## Install

```sh
npm install @voidzero-dev/vite-task-client
```

## Quick start

```js
import { ignoreInput, ignoreOutput, disableCache, getEnv } from '@voidzero-dev/vite-task-client';

ignoreInput('./node_modules/.cache/my-tool');

// `vp run` only exposes envs the task config declares; for everything
// else, `getEnv` fetches the value from the runner and registers it as
// a cache-key dependency in the same call.
const apiVersion = process.env.MY_API_VERSION ?? getEnv('MY_API_VERSION');

if (somethingNonDeterministicHappened) disableCache();
```

## Why this exists

`vp run` decides whether a task's cached result is reusable by hashing
everything your task read and everything it depends on. It infers that
set automatically — watching filesystem syscalls, scanning declared
inputs and env vars. That's safe but can be too coarse:

- Your tool maintains its own cache under `node_modules/.cache/…`. Every
  miss there would invalidate every other run, even though the contents
  don't actually affect your output.
- Your tool reads `process.env.MY_API_VERSION`, and `vp run`'s task
  config doesn't list it.
- Your tool has a non-deterministic mode it sometimes falls into and
  should skip the cache entirely.

These functions give you a precise way to correct each case from inside
your tool, without forcing your users to rewrite their `vp run` task
config.

## Reporting unchanged outputs

A tool that checks its own inputs can explicitly report that its outputs have
not changed:

```js
import { reportUnchanged } from '@voidzero-dev/vite-task-client';

if (await outputsAreUpToDate()) {
  reportUnchanged();
} else {
  await build();
}
```

The report applies to the current command, after it exits successfully. It does
not stop the process or change its exit code. Failures, cancellation, and
incomplete task communication discard the report. Repeated calls are harmless.
In a task with multiple commands, one command's report does not override the
others. The tool is responsible for checking that all its outputs are unchanged.

The runner displays `unchanged (reported by task)` separately from a cache hit.
Dependents still check their own inputs and cache entries: changed inputs,
missing cache entries, and disabled caching continue to require execution.
Directly requested tasks also retain their normal execution and cache behavior.

Successful reported runs can populate the cache using their current inputs.
When the command and cache configuration match an existing entry, the runner
keeps its output archive even if this execution did not rewrite any files.
Without a previous archive, outputs are collected normally; declare `output`
patterns for pre-existing outputs that automatic write tracing cannot discover.
An unreadable previous archive prevents the cache update and is reported as an
error, rather than silently losing restoration data.

For shell scripts, invoke `vp run --report-unchanged` **inside the script being
run** once its checks have established that its outputs are unchanged. For example,
the unchanged branch of a script can end with:

```sh
vp run --report-unchanged
exit 0
```

This CLI flag cannot be combined with a task name or other run options. It is a
successful no-op outside the runner, even outside a workspace. A configured but
unreachable runner produces an error. Rust tools can use
`vt_client::Client::report_unchanged()` on a client obtained from `Client::from_envs`.

A task written as `build && vp run --report-unchanged` has two separate commands;
the second command does not mark the preceding `build` as unchanged. Reports also
remain isolated across nested runner invocations.

`reportUnchanged()` is a no-op outside the runner and with older runner addons
that do not provide this method. Cache-disabled commands support reporting too.

## API

See [`src/index.d.ts`](./src/index.d.ts) for the full signatures and per-function
behavior.

## For `vite-task` developers

This package is a thin pure-JS wrapper with **no `dependencies` in
`package.json`** — its only runtime artifact is the napi addon, which
the runner provides at execution time.

That means when you change this package in a `vite-task` PR, the
consuming tool can pull your unpublished commit directly via a git URL
with a subpath, no npm release required:

```jsonc
// In the consuming tool's package.json
{
  "dependencies": {
    "@voidzero-dev/vite-task-client": "github:voidzero-dev/vite-task#<commit-sha>&path:/packages/vite-task-client",
  },
}
```

(`&path:` is supported by pnpm. For npm/yarn, see your package
manager's docs on monorepo-subpath git installs.)

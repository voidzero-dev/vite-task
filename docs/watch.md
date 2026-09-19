# Watching tasks

Use `vp run --watch build` to run a task immediately and run it again when its
inputs change. Put runner flags before the task name; `vp run build --watch`
passes `--watch` to the task itself.

```sh
vp run --watch build
vp run --watch --no-cache dev
vp run --watch -r dev
```

Watch mode uses the task's `input` configuration in `vite.config.*`. When `input`
is omitted, it discovers the files the command reads, including while a server
is still running. Automatic tracking also works with caching disabled. Explicit
patterns, exclusions, and workspace-relative patterns have the same meaning as
for cached tasks. Runner-aware `ignoreInput` calls exclude inferred inputs,
including descendants of ignored directories; explicit input patterns still
apply. An empty `input` array disables file-triggered restarts.

```ts
run: {
  tasks: {
    dev: {
      command: 'node server.js',
      cache: false,
      input: ['src/**', 'server.js', '!src/generated/**'],
    },
  },
}
```

Changes are collected for 100 ms. The affected task and its dependents restart;
unrelated tasks continue running. A task that is still running is stopped along
with its child processes before its next execution starts. Command arrays,
`&&` sequences, and script hooks restart from the beginning of their task.
Nested task invocations retain their dependency order and concurrency limits.
A nested `vp run --watch` remains an uncached child command and manages its own
input tracking.
`--parallel` removes execution ordering but keeps dependency-based invalidation.

Successful finite tasks can reuse cached results. Cache hits retain their input
subscriptions. Failed tasks wait for another input change, and their dependents
wait for a successful execution. Other independent tasks keep running. A task
interrupted for restart does not save a cache result.

File creation, removal, renaming, and atomic editor saves are supported. Reading
a directory's entries also tracks entry additions and removals. The runner
ignores its cache and `.git` paths. Automatically inferred writes do not trigger
the producing task; they can still trigger other tasks that read those outputs.
Exclude generated files from explicit inputs. An explicitly selected input that
the task also writes is reported as an error to prevent a restart loop.

Press Ctrl+C to stop watching and terminate the running tasks and descendants.
Watch mode closes task stdin and forwards stdout/stderr using the selected
`--log` mode. Grouped output remains grouped until the command finishes.

Task definitions, workspace membership, and dependency relationships are loaded
once. Restart the watch command after changing those settings. Long-running
servers still occupy concurrency slots, and a dependency must finish before
its dependent starts. Watch mode does not add service-readiness dependencies.
File access tracking covers workspace paths on macOS, Linux, and Windows. A
watcher failure or incomplete trace terminates the session with a diagnostic,
rather than silently ignoring changes.

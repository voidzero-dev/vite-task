# Changelog

- **Fixed** Windows file access tracking no longer panics when a task touches malformed paths that cannot be represented as workspace-relative inputs ([#330](https://github.com/voidzero-dev/vite-task/pull/330))
- **Fixed** `vp run --cache` now supports running without a task specifier and opens the interactive task selector, matching bare `vp run` behavior ([#312](https://github.com/voidzero-dev/vite-task/pull/313))
- **Fixed** Ctrl-C now prevents future tasks from being scheduled and prevents caching of in-flight task results ([#309](https://github.com/voidzero-dev/vite-task/pull/309))
- **Added** `--concurrency-limit` flag to limit the number of tasks running at the same time (defaults to 4) ([#288](https://github.com/voidzero-dev/vite-task/pull/288), [#309](https://github.com/voidzero-dev/vite-task/pull/309))
- **Added** `--parallel` flag to ignore task dependencies and run all tasks at once with unlimited concurrency (unless `--concurrency-limit` is also specified) ([#309](https://github.com/voidzero-dev/vite-task/pull/309))
- **Added** object form for `input` entries: `{ "pattern": "...", "base": "workspace" | "package" }` to resolve glob patterns relative to the workspace root instead of the package directory ([#295](https://github.com/voidzero-dev/vite-task/pull/295))
- **Fixed** arguments after the task name being consumed by `vp` instead of passed through to the task ([#286](https://github.com/voidzero-dev/vite-task/pull/286), [#290](https://github.com/voidzero-dev/vite-task/pull/290))
- **Changed** default untracked env patterns to align with Turborepo, covering more CI and platform-specific variables ([#262](https://github.com/voidzero-dev/vite-task/pull/262))
- **Added** `--log=interleaved|labeled|grouped` flag to control task output display: `interleaved` (default) streams directly, `labeled` prefixes lines with `[pkg#task]`, `grouped` buffers output per task ([#266](https://github.com/voidzero-dev/vite-task/pull/266))
- **Added** musl target support (`x86_64-unknown-linux-musl`) ([#273](https://github.com/voidzero-dev/vite-task/pull/273))
- **Changed** cache hit/miss indicators to use neutral symbols (◉/〇) instead of ✓/✗ to avoid confusion with success/error ([#268](https://github.com/voidzero-dev/vite-task/pull/268))
- **Added** automatic skip of caching for tasks that modify their own inputs ([#248](https://github.com/voidzero-dev/vite-task/pull/248))

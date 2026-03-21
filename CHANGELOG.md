# Changelog

## Unreleased

- **Added** `--log=interleaved|labeled|grouped` flag to control task output display: `interleaved` (default) streams directly, `labeled` prefixes lines with `[pkg#task]`, `grouped` buffers output per task ([#266](https://github.com/voidzero-dev/vite-task/pull/266))
- **Added** musl target support (`x86_64-unknown-linux-musl`) ([#273](https://github.com/voidzero-dev/vite-task/pull/273))
- **Added** automatic skip of caching for tasks that modify their own inputs ([#248](https://github.com/voidzero-dev/vite-task/pull/248))
- **Changed** cache hit/miss indicators to use neutral symbols (◉/〇) instead of ✓/✗ to avoid confusion with success/error ([#268](https://github.com/voidzero-dev/vite-task/pull/268))
- **Changed** default untracked env patterns to align with Turborepo, covering more CI and platform-specific variables ([#262](https://github.com/voidzero-dev/vite-task/pull/262))
- **Removed** `bootstrap-cli` command from the init task ([#261](https://github.com/voidzero-dev/vite-task/pull/261))
- **Fixed** flaky SIGSEGV on musl targets ([#278](https://github.com/voidzero-dev/vite-task/pull/278))

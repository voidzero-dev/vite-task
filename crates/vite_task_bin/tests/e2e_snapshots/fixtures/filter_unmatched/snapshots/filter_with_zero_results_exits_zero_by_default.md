# filter_with_zero_results_exits_zero_by_default

A `--filter` whose entire result is empty (typo, glob with no match, …) prints the warning and exits 0 — pnpm's default. Previously this errored with `Task "build" not found`.

## `vt run --filter nonexistent build`

```
No packages matched the filter: nonexistent
```

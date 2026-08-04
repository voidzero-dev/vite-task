# vt

This is the top-level library crate that orchestrates the full task lifecycle: loading the task graph, planning execution, running or replaying tasks , and reporting results.

It is consumed by two binaries:

- **[Vite+](https://github.com/voidzero-dev/vite-plus)** — the official product, where it powers `vp run`
- **[`vt_bin`](../vt_bin)** — internal `vt` CLI for developing and testing this repo without the full Vite+ stack

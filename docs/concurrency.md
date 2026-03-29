# Concurrency

By default, `vp run` executes up to 4 tasks at the same time while respecting the dependency order between them.

## `--concurrency-limit`

Override the maximum number of tasks that can run simultaneously:

```sh
vp run -t --concurrency-limit 1 build   # one at a time (sequential)
vp run -t --concurrency-limit 16 build  # up to 16 at once
```

**Why it defaults to 4**: Same as pnpm. It balances between enough concurrency to keep the machine busy and leaving headroom for the tasks themselves, which often consume all available CPU cores (e.g. `tsc`, bundlers).

**Why the name `--concurrency-limit`**: It conveys that this is **an upper bound, not a target**. We plan to introduce task-level concurrency control (e.g. a build task that already saturates all CPU cores), and when that lands the actual concurrency may be less than the limit.

Equivalent flags in other tools:

| pnpm                                                                               | Turborepo                                                                                    | Nx                                                                   |
| ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| [`--workspace-concurrency`](https://pnpm.io/cli/recursive#--workspace-concurrency) | [`--concurrency`](https://turborepo.dev/docs/reference/run#--concurrency-number--percentage) | [`--parallel`](https://nx.dev/nx-api/nx/documents/run-many#parallel) |

## `--parallel`

Run all matched tasks at the same time, ignoring dependency order:

```sh
vp run -r --parallel dev
```

**`--parallel` also removes the concurrency limit** (same as pnpm), which is useful for starting multiple dev servers that run indefinitely. To run tasks in parallel but still cap how many run at once, combine both flags:

```sh
vp run -r --parallel --concurrency-limit 8 lint
```

Equivalent flags in other tools:

| pnpm                                               | Turborepo                                                           | Nx  |
| -------------------------------------------------- | ------------------------------------------------------------------- | --- |
| [`--parallel`](https://pnpm.io/cli/run#--parallel) | [`--parallel`](https://turborepo.dev/docs/reference/run#--parallel) | n/a |

# Concurrency

`vp run` runs up to 4 tasks at once by default, respecting dependency order.

## `--concurrency-limit`/`VP_RUN_CONCURRENCY_LIMIT`

```sh
vp run -t --concurrency-limit 1 build   # sequential
vp run -t --concurrency-limit 16 build  # up to 16 at once
```

Also settable via the `VP_RUN_CONCURRENCY_LIMIT` env var. The CLI flag wins when both are present.

**Default is 4** (same as pnpm) — enough to keep the machine busy while leaving room for tasks that already use all cores (bundlers, `tsc`, etc.).

**Why the name `--concurrency-limit`**: The name makes clear this is an upper bound, not a target. We plan to add per-task concurrency control in `vite.config.*` (e.g. marking a build task as CPU-heavy), so the actual concurrency may end up lower than the limit.

**Why not support CPU percentage** (like Turborepo's `--concurrency 50%`): This setting is meant to be a simple upper bound. CPU-core-aware concurrency control belongs at the per-task level, which we plan to add in the future.

**Why support `VP_RUN_CONCURRENCY_LIMIT` env**: To allow people to apply it to every `vp run` without repeating the flag, especially in CI.

**Why not support `concurrencyLimit` in `vite.config.*`**: The right limit usually depends on the machine, not the project. We reserve config-file concurrency settings for per-task hints (see above).

Equivalent flags in other tools:

| pnpm                                                                               | Turborepo                                                                                    | Nx                                                                   |
| ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| [`--workspace-concurrency`](https://pnpm.io/cli/recursive#--workspace-concurrency) | [`--concurrency`](https://turborepo.dev/docs/reference/run#--concurrency-number--percentage) | [`--parallel`](https://nx.dev/nx-api/nx/documents/run-many#parallel) |

## `--parallel`

Ignores dependency order and removes the concurrency limit:

```sh
vp run -r --parallel dev
```

Useful for starting dev servers that all need to run at the same time. Same behavior as `--parallel` in pnpm.

To ignore dependency order but still cap concurrency, combine both flags:

```sh
vp run -r --parallel --concurrency-limit 8 lint
```

Equivalent flags in other tools:

| pnpm                                               | Turborepo                                                           | Nx  |
| -------------------------------------------------- | ------------------------------------------------------------------- | --- |
| [`--parallel`](https://pnpm.io/cli/run#--parallel) | [`--parallel`](https://turborepo.dev/docs/reference/run#--parallel) | n/a |

## Resolution order of concurrency limit

The concurrency limit is resolved in this order (first match wins):

1. `--concurrency-limit` CLI flag
2. `--parallel` without the above → unlimited
3. `VP_RUN_CONCURRENCY_LIMIT` env var
4. Default: 4

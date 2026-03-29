# Concurrency

By default, `vp run` executes up to 4 tasks at the same time while respecting the dependency order between them.

## `--concurrency-limit`

Override the number of tasks that can run simultaneously:

```sh
vp run -r build --concurrency-limit 1   # one at a time (sequential)
vp run -r build --concurrency-limit 16  # up to 16 at once
```

## `--parallel`

Run all matched tasks at the same time, ignoring dependency order:

```sh
vp run -r lint --parallel
```

This is useful for tasks like linting or type-checking that don't depend on each other's output.

`--parallel` also removes the concurrency limit. To run tasks in parallel but still cap how many run at once, combine both flags:

```sh
vp run -r lint --parallel --concurrency-limit 8
```

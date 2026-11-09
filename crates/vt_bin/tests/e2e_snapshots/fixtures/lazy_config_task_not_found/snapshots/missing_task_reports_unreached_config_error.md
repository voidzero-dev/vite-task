# missing_task_reports_unreached_config_error

Bug: `packages/broken/vite-task.json` sets `cache`, which only the workspace root config may set. `vt run build` in `packages/app` never reads `@test/broken`'s config, but `vt run ship`, a task `@test/app` doesn't define, does: the run falls back to listing every task, so it fails with `@test/broken`'s error instead of saying `Task "ship" not found`. With `--parallel`, which skips that fallback, the same command gets the not-found error.

## `vt run build`

never reads @test/broken's config

```
~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app
```

## `vtt pipe-stdout -- vt run ship`

bug: fails with @test/broken's error instead of `Task "ship" not found.`

**Exit code:** 1

```
error: `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
--- stdout ---
```

## `vt run ship`

bug: in a terminal, fails with the same error instead of showing the task selector

**Exit code:** 1

```
error: `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vt run --parallel ship`

the same task name without the fallback is reported as not found

**Exit code:** 1

```
error: Task "ship" not found
```

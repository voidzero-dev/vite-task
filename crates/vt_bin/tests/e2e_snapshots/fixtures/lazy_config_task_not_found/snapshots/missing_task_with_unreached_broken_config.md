# missing_task_with_unreached_broken_config

`packages/broken/vite-task.json` sets `cache`, which only the workspace root config may set. `vt run build` in `packages/app` never reads `@test/broken`'s config. Neither should `vt run ship`, a task `@test/app` doesn't define, but it falls back to listing every task, so it fails with `@test/broken`'s error instead of saying `Task "ship" not found`. With `--parallel`, which skips that fallback, the same command gets the not-found error.

## `vt run build`

never reads @test/broken's config

```
~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app
```

## `vtt pipe-stdout -- vt run ship`

fails with @test/broken's error instead of `Task "ship" not found.`

**Exit code:** 1

```
error: `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
--- stdout ---
```

## `vt run ship`

in a terminal, fails with the same error

**Exit code:** 1

```
error: `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vt run --parallel ship`

without the fallback, the same command is reported as not found

**Exit code:** 1

```
error: Task "ship" not found
```

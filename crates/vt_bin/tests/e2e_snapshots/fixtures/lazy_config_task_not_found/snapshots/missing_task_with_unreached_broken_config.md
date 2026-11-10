# missing_task_with_unreached_broken_config

`packages/broken/vite-task.json` sets `cache`, which only the workspace root config may set. `vt run build` in `packages/app` never reads `@test/broken`'s config. `vt run ship`, a task `@test/app` doesn't define, falls back to listing every task, which loads `@test/broken`'s config. When that fails, it reports `Task "ship" not found`, as `--parallel`, which skips the fallback, does, instead of `@test/broken`'s error.

## `vt run build`

never reads @test/broken's config

```
~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app
```

## `vtt pipe-stdout -- vt run ship`

not found, without @test/broken's error

**Exit code:** 1

```
error: Task "ship" not found
--- stdout ---
```

## `vt run ship`

in a terminal, also not found, without the task selector

**Exit code:** 1

```
error: Task "ship" not found
```

## `vt run --parallel ship`

without the fallback, also not found

**Exit code:** 1

```
error: Task "ship" not found
```

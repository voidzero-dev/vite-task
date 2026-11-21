# cache_set_in_unreached_package

`packages/broken/vite-task.json` sets `cache`, which only the workspace root config may set. `vt run -t build` in `packages/app` runs `@test/app` and `@test/lib`, so it never needs `@test/broken`'s config; `-r` and `dependsOn` reach it.

## `vt run -t build`

app and lib only

```
~/packages/lib$ echo 'Building lib' ⊘ cache disabled
Building lib

~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app

---
vt run: 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vt run -r build`

reaches @test/broken

**Exit code:** 1

```
error: Failed to load task graph
* `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vt run deploy`

`dependsOn` reaches @test/broken

**Exit code:** 1

```
error: Failed to load task graph
* `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vtt pipe-stdout -- vt run --json`

`--json` loads every config, as listing the tasks does

**Exit code:** 1

```
error: `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
--- stdout ---
```

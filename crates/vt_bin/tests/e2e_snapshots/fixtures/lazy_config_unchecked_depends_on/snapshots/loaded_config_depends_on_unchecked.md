# loaded_config_depends_on_unchecked

Bug: `@test/app#lint` depends on `typecheck`, which `@test/app` doesn't define, and the root's `release` depends on `@test/nope#publish`, though no package is named `@test/nope`. Neither check needs another package's config. `vt run build` in `packages/app` loads both configs, since the root's always loads, but checks a `dependsOn` only when its task runs, so it succeeds without reporting either. `vt run`, which lists every task, doesn't report them either, while `vt run --json`, which loads the same configs, does.

## `vt run build`

bug: loads the root's and @test/app's configs, but reports neither `dependsOn` error

```
~/packages/app$ echo 'Building app' ⊘ cache disabled
Building app
```

## `vtt pipe-stdin -- vt run`

bug: lists every task, including both with a broken `dependsOn`, without an error

```
  build: echo 'Building app'
  lint: echo 'Linting app'
  lazy-config-unchecked-depends-on#release: echo 'Releasing'
```

## `vtt pipe-stdout -- vt run --json`

loads the same configs and reports the error

**Exit code:** 1

```
error: Failed to lookup dependency 'typecheck' for task @test/app#lint
* Task 'typecheck' not found in package @test/app
--- stdout ---
```

## `vt run lint`

the error shows up only when the task runs

**Exit code:** 1

```
error: Failed to load task graph
* Failed to lookup dependency 'typecheck' for task @test/app#lint
* Task 'typecheck' not found in package @test/app
```

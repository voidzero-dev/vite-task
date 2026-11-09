# depends_on_load_error_has_no_context

Bug: `packages/broken/vite-task.json` sets `cache`, which only the workspace root config may set. Most runs never read it, so when one fails with its error, the error has to say what reached `@test/broken`. A nested `vt run` says so, and so does a `dependsOn` entry naming a missing task, but a `dependsOn` entry that loads `@test/broken` doesn't: `vt run deploy` fails with `@test/broken`'s error alone, without naming `@test/app#deploy` or its `@test/broken#build` entry.

## `vt run deploy`

bug: doesn't say that the `dependsOn` of @test/app#deploy loaded @test/broken

**Exit code:** 1

```
error: Failed to load task graph
* `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vt run release`

a nested `vt run` names the task and the command

**Exit code:** 1

```
error: Failed to plan tasks from `vt run --filter @test/broken build` in task @test/app#release
* Failed to load task graph
* `cache` can only be set in the workspace root config, but found in <workspace>/packages/broken
```

## `vt run publish`

a `dependsOn` entry naming a missing task names the task and the entry

**Exit code:** 1

```
error: Failed to load task graph
* Failed to lookup dependency '@test/lib#missing' for task @test/app#publish
* Task 'missing' not found in package @test/lib
```

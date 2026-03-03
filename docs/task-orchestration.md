# Task Orchestration

This document explains how Vite Task determines which tasks to run and in what order.

## Two Kinds of Dependencies

### 1. Topological Dependencies (from `package.json`)

These are implicit. When you run a task across multiple packages (with `-r` or `-t`), Vite Task looks at `package.json` `dependencies` and `devDependencies` to determine the order. If `@my/app` depends on `@my/lib`, and `@my/lib` depends on `@my/core`, then `build` runs in order: core → lib → app.

**Example workspace:**

```json
// packages/core/package.json
{ "name": "@my/core" }

// packages/lib/package.json
{ "name": "@my/lib", "dependencies": { "@my/core": "workspace:*" } }

// packages/app/package.json
{ "name": "@my/app", "dependencies": { "@my/lib": "workspace:*" } }
```

Each package has a `"build": "echo 'Building <name>'"` script.

```
> vp run -r build

~/packages/core$ echo 'Building core'
Building core

~/packages/lib$ echo 'Building lib'
Building lib

~/packages/app$ echo 'Building app'
Building app
```

The execution plan looks like:

```
┌───────────┐
│ core#build│
└─────┬─────┘
      │
┌─────▼─────┐
│ lib#build │
└─────┬─────┘
      │
┌─────▼─────┐
│ app#build │
└───────────┘
```

Topological ordering is **automatically enabled** when using `-r` (recursive) or `-t` (transitive). It is **not applied** for single-package runs.

### 2. Explicit Dependencies (`dependsOn`)

These are declared in your config and represent task-level dependencies — a task that must complete before another task starts:

```ts
// vite.config.ts
export default defineConfig({
  run: {
    tasks: {
      deploy: {
        command: 'deploy-script --prod',
        cache: false,
        dependsOn: ['@my/app#build', '@my/app#test', '@my/utils#lint'],
      },
    },
  },
});
```

The execution plan for `vp run deploy` (from the app package):

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  app#build   │  │  app#test    │  │  utils#lint  │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
       └─────────────────┼─────────────────┘
                         │
                  ┌──────▼───────┐
                  │  app#deploy  │
                  └──────────────┘
```

Explicit dependencies are **always applied** (unless `--ignore-depends-on` is passed).

The `dependsOn` format is `[package#]taskName`:

- `"build"` — the `build` task in the same package
- `"@my/app#build"` — the `build` task in a specific package
- `"lint"` — the `lint` task in the same package

**Important:** Explicit dependencies can pull in tasks from packages _outside_ the current selection. In the example above, even if you only selected `@my/app`, the `@my/utils#lint` task is pulled in because `deploy` explicitly depends on it.

### Both Combined

In a recursive run, both dependency types apply simultaneously. Given:

```ts
export default defineConfig({
  run: {
    tasks: {
      lint: {
        command: 'eslint src',
        dependsOn: ['clean'], // lint depends on clean in same package
      },
    },
  },
});
```

Running `vp run -r lint`:

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  core#clean  │  │  utils#clean │  │  app#clean   │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
┌──────▼───────┐  ┌──────▼───────┐         │
│  core#lint   │  │  utils#lint  │         │
└──────┬───────┘  └──────┬───────┘         │
       │                 │                 │
       └─────────────────┼─────────────────┘
                         │
                  ┌──────▼───────┐
                  │   app#lint   │
                  └──────────────┘
```

Here, topological order (core#lint and utils#lint before app#lint) combines with explicit deps (clean before lint in each package). Notice that `app#clean` can start immediately — it doesn't wait for upstream packages. Only `app#lint` waits for both its explicit dependency (`app#clean`) and its topological dependencies (`core#lint`, `utils#lint`).

## Skip-Intermediate Reconnection

When running a task recursively or transitively, some packages in the dependency chain might not have that task. Vite Task handles this gracefully by "bridging" across the gap.

**Example:**

```
packages/top      → depends on packages/middle
packages/middle   → depends on packages/bottom
```

- `top` has a `build` script
- `middle` does NOT have a `build` script
- `bottom` has a `build` script

Running `vp run -t build` from `packages/top`:

```
┌───────────────┐
│ bottom#build  │       middle is skipped — it has no build task
└───────┬───────┘
        │
┌───────▼───────┐
│  top#build    │
└───────────────┘
```

The dependency chain `top → middle → bottom` is preserved as `top → bottom` for the `build` task, with `middle` transparently skipped. This means the topological ordering is still correct: `bottom#build` runs before `top#build`.

## Compound Commands

Commands joined with `&&` are split into independently-cached sub-tasks that run sequentially within the same task:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc && rollup -c',
      },
    },
  },
});
```

```
> vp run build
$ tsc
... tsc output ...

$ rollup -c
... rollup output ...

---
[vp run] 0/2 cache hit (0%).
```

Each sub-task has its own cache entry. On the next run, if only `rollup.config.js` changed:

```
> vp run build
$ tsc ✓ cache hit, replaying
... tsc output ...

$ rollup -c ✗ cache miss: content of input 'rollup.config.js' changed, executing
... rollup output ...

---
[vp run] 1/2 cache hit (50%), 2.3s saved.
```

## Nested `vp run` Expansion

When a task script contains a `vp run` call, it is **expanded at plan time** — not spawned as a separate child process. The planner parses the nested command and incorporates its tasks directly into the execution graph.

```json
// package.json (workspace root)
{
  "scripts": {
    "ci": "vp run lint && vp run test && vp run build"
  }
}
```

Running `vp run ci` from the root package expands to:

```
┌──────────────┐
│   #lint      │  (expanded from "vp run lint")
└──────┬───────┘
       │
┌──────▼───────┐
│   #test      │  (expanded from "vp run test")
└──────┬───────┘
       │
┌──────▼───────┐
│   #build     │  (expanded from "vp run build")
└──────────────┘
```

This expansion is recursive — nested `vp run` calls within nested calls are also expanded. Features like `--filter` and `-r` within nested scripts work correctly:

```json
{
  "scripts": {
    "build-all": "vp run -r build"
  }
}
```

Running `vp run build-all` expands `vp run -r build` at plan time, producing tasks for every package.

### Recursive Self-Reference Handling\*

A common pattern is having the workspace root orchestrate recursive builds:

```json
// Workspace root package.json
{
  "scripts": {
    "build": "vp run -r build"
  }
}
```

This creates a potential recursion: root's `build` → `vp run -r build` → includes root's `build` → ...

Vite Task handles this with two rules:

**Rule 1 — Skip duplicate commands.** When a task's command is the **same `vp run` invocation** already running, that command is skipped:

```bash
$ vp run -r build
# root#build's command is "vp run -r build" — identical to the current invocation
# → command skipped. root#build becomes a passthrough.
# Other packages' build tasks run normally.
```

**Rule 2 — Prune self from nested expansions.** When a task's command expands to a **different** `vp run` invocation whose results include the task itself, the self-reference is removed:

```bash
$ vp run build      # (from workspace root)
# root#build's command "vp run -r build" is different from "vp run build"
# → expanded normally, but root#build is pruned from the expansion results.
# Result: root#build orchestrates other packages' build tasks.
```

Multi-command scripts work naturally — only the matching subcommand is skipped:

```json
{
  "scripts": {
    "build": "tsc && vp run -r build"
  }
}
```

`tsc` runs, and the recursive `vp run -r build` is skipped (Rule 1). The `dependsOn` edges through the passthrough still work.

**Note:** Mutual recursion through different tasks (e.g., `build` → `vp run -r test` → `vp run -r build`) remains a fatal error.

## `--ignore-depends-on`

You can skip all explicit `dependsOn` edges for a run:

```bash
vp run -r build --ignore-depends-on
```

This runs `build` across all packages respecting only the topological (package.json) dependency order — ignoring any `dependsOn` declarations. Useful when you know dependencies are already satisfied and want a faster run.

## Execution Order Visualization

For a workspace with this package structure:

```
@my/core        (no dependencies)
@my/utils       (no dependencies)
@my/lib         → depends on @my/core
@my/cli         → depends on @my/core
@my/app         → depends on @my/lib, @my/utils
```

### `vp run -r build` (recursive)

All packages, topological order:

```
┌──────────────┐                      ┌──────────────┐
│  core#build  │                      │ utils#build  │
└──────┬───────┘                      └──────┬───────┘
       │                                     │
       ├──────────────────┐                  │
       │                  │                  │
┌──────▼───────┐   ┌──────▼───────┐          │
│  lib#build   │   │  cli#build   │          │
└──────┬───────┘   └──────────────┘          │
       │                                     │
       └──────────────┬──────────────────────┘
                      │
               ┌──────▼───────┐
               │  app#build   │
               └──────────────┘
```

Execution plan (dependencies → dependents):

```json
{
  "core#build": [],
  "utils#build": [],
  "lib#build": ["core#build"],
  "cli#build": ["core#build"],
  "app#build": ["lib#build", "utils#build"]
}
```

### `vp run -t build` (transitive, from app/)

Only `@my/app` and its transitive dependencies:

```
┌──────────────┐     ┌──────────────┐
│  core#build  │     │ utils#build  │
└──────┬───────┘     └──────────┬───┘
       │                        │
┌──────▼───────┐                │
│  lib#build   │                │
└──────┬───────┘                │
       │                        │
       └────────┬───────────────┘
                │
         ┌──────▼───────┐
         │  app#build   │
         └──────────────┘
```

Notice `cli#build` is not included — it's not a dependency of `@my/app`.

### `vp run -t build` (transitive, from lib/)

Only `@my/lib` and its transitive dependencies:

```
┌──────────────┐
│  core#build  │
└──────┬───────┘
       │
┌──────▼───────┐
│  lib#build   │
└──────────────┘
```

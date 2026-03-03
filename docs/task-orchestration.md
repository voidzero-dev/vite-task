# Task Orchestration

This document explains how Vite Task determines which tasks to run and in what order.

## Two Kinds of Dependencies

### 1. Topological Dependencies (from `package.json`)

These are implicit. When you run a task across multiple packages (with `-r` or `-t`), Vite Task uses `package.json` `dependencies` and `devDependencies` to determine the order.

```
> vp run -r build

~/packages/core$ tsc        # @my/core — no dependencies, runs first
~/packages/lib$ tsc         # @my/lib — depends on @my/core
~/packages/app$ tsc         # @my/app — depends on @my/lib
```

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

Explicit dependencies are **always applied**, unless you pass `--ignore-depends-on` to skip them and rely only on topological ordering.

The `dependsOn` format is `[package#]taskName`:

- `"build"` — the `build` task in the same package
- `"@my/app#build"` — the `build` task in a specific package
- `"lint"` — the `lint` task in the same package

**Important:** Explicit dependencies can pull in tasks from packages _outside_ the current selection. In the example above, even if you only selected `@my/app`, the `@my/utils#lint` task is pulled in because `deploy` explicitly depends on it.

### Both Combined

In a recursive run, both dependency types apply simultaneously. Topological edges connect the same task across packages, while `dependsOn` edges connect different tasks within a package. They're independent — a `dependsOn` target in a downstream package can start immediately without waiting for upstream packages.

## Compound Commands

Commands joined with `&&` follow standard bash semantics — they run sequentially and short-circuit on failure. Vite Task splits them into independently-cached sub-tasks:

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

When a task script contains a `vp run` call, it is **expanded at plan time** — not spawned as a separate child process. The planner parses the nested command and incorporates its tasks directly into the execution graph. This is fundamentally different from how other task runners handle nested invocations, and it unlocks several benefits:

- **Full visibility** — the execution plan shows every task that will run, even through layers of nesting
- **Per-task caching** — each expanded task is cached independently
- **No process overhead** — no extra `vp` processes are spawned

### Basic Expansion

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

Each `vp run` call is replaced with the actual tasks it would produce. The `&&` between them preserves sequential ordering — `#test` won't start until `#lint` finishes.

### Flags Work Inside Nested Scripts

All CLI flags (`-r`, `-t`, `--filter`) are parsed and evaluated during expansion:

```json
{
  "scripts": {
    "build-all": "vp run -r build",
    "test-app": "vp run --filter @my/app... test"
  }
}
```

`vp run build-all` expands `vp run -r build` at plan time, producing individual `build` tasks for every package in topological order — as if you'd run `vp run -r build` directly.

`vp run test-app` expands `vp run --filter @my/app... test` and produces `test` tasks for `@my/app` and all its transitive dependencies.

### Expansion is Recursive

Nesting works through multiple levels. If `script-a` calls `vp run script-b` and `script-b` calls `vp run script-c`, all layers are expanded into a single flat execution graph at plan time.

### Compound Commands with Nested Expansion

Compound commands (`&&`) and nested `vp run` interact naturally. Each segment is processed independently:

```json
{
  "scripts": {
    "release": "vp run -r build && deploy-script --prod"
  }
}
```

This expands into:

1. All package `build` tasks from `vp run -r build` (expanded, cached individually)
2. Then `deploy-script --prod` (run as a normal command)

The `build` tasks benefit from per-task caching — if only one package changed, only that package rebuilds. The `deploy-script` always runs after all builds complete.

### Working Directory Behavior

Following standard bash semantics, `cd` affects the cwd of all subsequent segments.

```json
{
  "scripts": {
    "test-src": "cd src && vp lint"
  }
}
```

Here `vp lint` runs with cwd set to `src/`.

> **Note:** `vp run` expansions always run tasks in the package root regardless of the current cwd — the expanded task's cwd comes from the task graph, not the shell. For example, `cd src && vp run build` still runs `build` in the package root.

### Cache Independence

Each expanded task retains its own cache configuration. A parent task disabling caching doesn't affect the expanded children:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        cache: true,
      },
      deploy: {
        command: 'vp run build && deploy-script',
        cache: false,
      },
    },
  },
});
```

Running `vp run deploy`: the `deploy` task itself isn't cached, but the expanded `build` task inside it still uses caching. If `build` has a cache hit, `tsc` is skipped even though it's invoked through a non-cached parent.

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

This creates a potential recursion: root's `build` → `vp run -r build` → includes root's `build` → ... Vite Task detects this and prunes the self-reference so other packages' builds run normally:

```json
{
  "scripts": {
    "build": "tsc && vp run -r build"
  }
}
```

```
> vp run -r build

~/$ tsc                            # root's own build step runs
...
~/packages/core$ tsc               # other packages' build tasks run
...
~/packages/lib$ tsc
...
# root's "vp run -r build" is pruned — no infinite loop
```

Cycles across different tasks (e.g., `build` calls `vp run -r test` which calls `vp run -r build`) are also detected statically at plan time — Vite Task will report an error rather than hang.

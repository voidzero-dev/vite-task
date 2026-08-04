# Task Query

How `vp run` decides which tasks to run and in what order.

## The two things we build

When `vp` starts, it builds two data structures from the workspace:

1. **Package graph** — which packages depend on which. Built from `package.json` dependency fields.
2. **Task graph** — which tasks exist and their explicit `dependsOn` relationships. Built from `vite.config.*` and `package.json` scripts.

Both are built once and reused for every query, including nested `vp run` calls inside task scripts.

### What goes into the task graph

The task graph contains a node for every task in every package, and edges only for explicit `dependsOn` declarations.

```jsonc
// packages/app/vite.config.*
{
  "tasks": {
    "build": {
      "command": "vite build",
      "dependsOn": ["@shared/lib#build"], // ← this becomes an edge
    },
  },
}
```

```
Task graph:

  app#build ──dependsOn──> lib#build
  app#test
  lib#build
  lib#test
```

Package dependency ordering (app depends on lib) is NOT stored as edges in the task graph. Why not is explained below.

Object-form `dependsOn` entries are also explicit task dependencies. At startup,
they are resolved against the declaring package's direct `package.json`
dependency fields and materialized as task graph edges:

```jsonc
// packages/app/vite.config.*
{
  "tasks": {
    "test": {
      "command": "vitest run",
      "dependsOn": [{ "task": "build", "from": ["dependencies", "devDependencies"] }],
    },
  },
}
```

If `app` directly depends on `ui` and `shared`, and both packages have `build`,
the task graph contains:

```
app#test ──dependsOn──> ui#build
app#test ──dependsOn──> shared#build
```

Dependency packages without the requested task are skipped. Recursive expansion
comes from dependency tasks declaring their own `dependsOn` entries.

## What happens when you run a query

Every `vp run` command goes through two stages:

```
Stage 1: Which packages?     Stage 2: Which tasks?

  package graph                 task graph
  + CLI flags          ──>      + package subgraph
  ─────────────                 + task name
  = package subgraph            ─────────────────
                                = execution plan
```

### Stage 1: Package selection

The CLI flags determine which packages participate:

| Command                      | What it selects                               |
| ---------------------------- | --------------------------------------------- |
| `vp run build`               | Just the current package                      |
| `vp run -r build`            | All packages                                  |
| `vp run -t build`            | Current package + its transitive dependencies |
| `vp run -w build`            | The workspace root package                    |
| `vp run -F app... build`     | `app` + its transitive dependencies           |
| `vp run -F '!core' -r build` | All packages except `core`                    |

The result is a **package subgraph** — the selected packages plus all the dependency edges between them. This subgraph is a subset of the full package graph.

### Stage 2: Task mapping

Given the package subgraph and a task name, we build the execution plan:

1. Find which selected packages have the requested task.
2. For packages that don't have it, reconnect their predecessors to their successors (skip-intermediate, explained below).
3. Map the remaining package nodes to task nodes — this gives us topological ordering.
4. Follow explicit `dependsOn` edges outward from these tasks (may pull in tasks from outside the selected packages).

The result is the execution plan: which tasks to run and in what order.

## Why topological edges aren't stored in the task graph

Consider this workspace:

```
Package graph:          Tasks each package has:

  app ──> lib ──> core    app:  build, test
                          lib:  build, test
                          core: build, test
```

If we pre-computed topological edges for `build`, the task graph would have:

```
app#build ──> lib#build ──> core#build
```

This looks fine for `vp run -r build`. But what about `vp run --filter app --filter core build` (selecting just app and core, skipping lib)?

The pre-computed edges say `app#build → lib#build → core#build`. But lib isn't selected — so we'd need `app#build → core#build`. That edge doesn't exist in the pre-computed graph. We'd have to recompute it anyway.

It gets worse. If lib didn't have a `build` task at all, the pre-computed edges would already skip it: `app#build → core#build`. But if you ran `vp run --filter app --filter lib build`, you'd want `app#build → lib#build` — which conflicts with the pre-computed skip.

The problem is that "which packages are selected" is a per-query decision, and skip-intermediate reconnection depends on that selection. Pre-computed topological edges encode a single global answer that doesn't work for all queries.

Instead, we compute topological ordering at query time from the package subgraph. The package subgraph already has the right set of packages and edges for the specific query. We just need to map packages to tasks and handle the ones that lack the requested task.

## Skip-intermediate reconnection

When a selected package doesn't have the requested task, we bridge across it.

### Example: middle package lacks the task

```
Package subgraph (from --filter top...):

  top ──> middle ──> bottom

Tasks:
  top:    has "build"
  middle: no "build"
  bottom: has "build"
```

Step by step:

1. `top` has `build` → keep it.
2. `middle` has no `build` → connect its predecessors (`top`) directly to its successors (`bottom`), then remove `middle`.
3. `bottom` has `build` → keep it.

```
Before reconnection:     After reconnection:

  top ──> middle ──> bottom    top ──> bottom

Task execution plan:

  top#build ──> bottom#build
```

`bottom#build` runs first, then `top#build`.

### Example: entry package lacks the task

```
Package subgraph (from --filter middle...):

  middle ──> bottom

Tasks:
  middle: no "build"
  bottom: has "build"
```

`middle` lacks `build`, so we reconnect. It has no predecessors, so there's nothing to bridge. We just remove it.

```
Task execution plan:

  bottom#build
```

Only `bottom#build` runs.

### Mutating the subgraph during reconnection

The package subgraph is already a lightweight `DiGraphMap<PackageNodeIndex, ()>` — just node indices and edges, not a copy of the full package graph. But reconnection adds bridge edges and removes nodes, and we need those edits to be visible within the same pass. If two consecutive packages lack the task, the second removal needs to see the bridge edge from the first.

So we clone the `DiGraphMap` once and mutate the clone. We iterate the original (stable node order) while modifying the clone.

## Explicit dependency expansion

After mapping the package subgraph to tasks, we follow explicit `dependsOn` edges from the task graph. This can pull in tasks from packages outside the selected set.

```jsonc
// packages/app/vite.config.*
{
  "tasks": {
    "build": {
      "dependsOn": ["codegen#generate"],
    },
  },
}
```

If you run `vp run --filter app build`, the package subgraph contains only `app`. But `app#build` has a `dependsOn` pointing to `codegen#generate`. The expansion step follows this edge and adds `codegen#generate` to the execution plan, even though `codegen` wasn't in the filter.

This is intentional — `dependsOn` is an explicit declaration that a task can't run without its dependency. Ignoring it would break the build. (Users can skip this with `--ignore-depends-on`.)

The expansion follows explicit `dependsOn` edges, including edges materialized from object-form entries. It does not follow topological package edges. Topological ordering comes from the package subgraph — it's already baked into the task execution graph by Stage 2.

## Nested `vp run`

A task script can contain `vp run` calls:

```jsonc
{
  "tasks": {
    "ci": {
      "command": "vp run -r build && vp run -r test",
    },
  },
}
```

Each nested `vp run` goes through the same two stages. It reuses the same package graph and task graph that were built at startup — no reloading.

The nested query produces its own execution subgraph, which gets embedded inside the parent task's execution plan as an expanded item.

## Putting it all together

```
Startup (once):
  workspace files ──> package graph ──> task graph
                      (dependencies)    (tasks + dependsOn edges)

Per query:
  CLI flags ──> PackageQuery
                    │
                    ▼
  package graph ──> package subgraph (selected packages + edges)
                    │
                    ▼
  task graph ────> task execution graph
                   (map packages to tasks,
                    skip-intermediate reconnection,
                    explicit dep expansion)
                    │
                    ▼
                   execution plan
                   (resolve env vars, commands, cwd,
                    expand nested vp run calls)
```

The package graph and task graph are stable. They don't change between queries. Everything query-specific is derived from them on the fly.

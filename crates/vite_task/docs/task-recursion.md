# Task Recursion — Implementation Detail

This document describes the implementation of the two recursion-handling rules
proposed in [workspace-root.md](workspace-root.md).

## Two rules

### Rule 1 — Skip duplicate commands

When a task's command contains a `vp run` invocation, the planner parses it and
expands it inline. If the parsed query is identical to the query already being
expanded, the subcommand is skipped — it would only duplicate work that the
parent expansion is already doing.

**When it fires:** `vp run -r build` → root#build has command `vp run -r build`
→ same query → skip.

### Rule 2 — Prune self from nested expansions

When a task's command expands to a **different** query, that expansion proceeds
normally. But if the resulting task set includes the task currently being
planned, that task is removed from the expansion (and its dependency edges are
reconnected, same as when a package lacks the requested task).

**When it fires:** `vp run build` (single package, from root) → root#build has
command `vp run -r build` (different query) → expanded → result includes
root#build → pruned from expansion.

## Where the checks happen

### Rule 1

In `plan_task_as_execution_node`, each `&&`-separated subcommand is parsed via
`PlanRequestParser::get_plan_request`. When the result is a
`PlanRequest::Query(inner)`, compare `inner.query` against the parent query
stored in `PlanContext`. If they match, skip this subcommand (don't call
`plan_query_request`, don't add an `ExecutionItem`).

```
plan_query_request(query: "-r build", ctx)
  │
  ├─ plan root#build
  │    command: "vp run -r build"
  │    parsed:  Query { task: "build", recursive: true }
  │    compare: same as parent query → SKIP (rule 1)
  │    result:  root#build has no execution items (passthrough)
  │
  ├─ plan a#build
  │    command: "tsc --noEmit"
  │    parsed:  None (external command)
  │    result:  leaf spawn execution
  │
  └─ plan b#build
       command: "tsc --noEmit"
       parsed:  None (external command)
       result:  leaf spawn execution
```

### Rule 2

In `plan_query_request`, after the nested query is expanded into a task node
graph, check each node against the `task_call_stack` in `PlanContext`. If a node
matches the immediate parent (the task whose command triggered this expansion),
remove it from the graph and reconnect edges.

```
plan_task_as_execution_node(root#build, ctx)    ← called from top-level "vp run build"
  │  command: "vp run -r build"
  │  parsed:  Query { task: "build", recursive: true }
  │  compare: different from parent query ("build", no -r) → EXPAND
  │
  └─ plan_query_request(query: "-r build", ctx)
       │  expanded task set: [root#build, a#build, b#build]
       │  root#build is on the call stack → PRUNE (rule 2)
       │  final task set: [a#build, b#build]
       │
       ├─ plan a#build → leaf spawn execution
       └─ plan b#build → leaf spawn execution
```

## Changes to PlanContext

Add one field:

```rust
/// The query that caused the current expansion, if any.
/// Used by rule 1 to detect and skip duplicate nested expansions.
parent_query: Option<Arc<TaskQuery>>,
```

- `plan_query_request` sets this before planning child nodes.
- `plan_task_as_execution_node` reads it when a subcommand parses as a
  `PlanRequest::Query`.
- `duplicate()` clones it along with the rest of the context.

Rule 2 uses the existing `task_call_stack` — no new state needed.

## Comparison semantics (rule 1)

The comparison is on `TaskQuery` (the package selection + task name). This
includes:

- Task name (`build`, `test`, etc.)
- Package selection mode (`-r`, `-t`, specific package, `--filter`)
- Filter patterns

Extra args (`-- --verbose`) are part of `PlanOptions`, not `TaskQuery`. Two
invocations with the same query but different extra args are **not** considered
duplicates — the inner invocation is trying to do something different and should
expand (or hit rule 2 / existing recursion detection if it cycles).

`TaskQuery` needs to derive `PartialEq`.

## Interaction with existing recursion detection

The three checks operate at different levels:

| Check                         | Level           | When                               | What it catches                                                                              |
| ----------------------------- | --------------- | ---------------------------------- | -------------------------------------------------------------------------------------------- |
| Rule 1 (duplicate query skip) | Subcommand      | Before expanding a nested `vp run` | Same query re-entered (e.g., `-r build` inside `-r build`)                                   |
| Rule 2 (self-pruning)         | Task node graph | After expanding a nested `vp run`  | Same task in a different query's expansion (e.g., `build` → `-r build` includes self)        |
| `check_recursion`             | Task node       | Before planning each task          | Same task node on the call stack from a non-prunable path (e.g., mutual recursion A → B → A) |
| Cycle detection               | Execution graph | After all nodes are planned        | Cycles in the final `dependsOn` / topological edge graph                                     |

Rule 1 prevents the most common case (self-referential root scripts invoked
with `-r`) from ever reaching deeper checks. Rule 2 handles the single-package
invocation case. Mutual recursion through different tasks still hits
`check_recursion` and remains a fatal error.

## Multi-command scripts

For `&&`-chained commands, each subcommand is checked independently (rule 1):

```
command: "tsc && vp run -r build && echo done"

subcommand 1: "tsc"              → external command, runs normally
subcommand 2: "vp run -r build"  → matches parent query → SKIPPED
subcommand 3: "echo done"        → builtin, runs normally
```

The task node still has execution items from the non-skipped subcommands (`tsc`,
`echo done`). It is not a pure passthrough in this case.

## Edge case: dependsOn through a passthrough node

```jsonc
{ "tasks": { "build": { "command": "vp run -r build", "dependsOn": ["lint"] } } }
```

`dependsOn` edges are resolved in the task query graph, independent of command
expansion. When root#build's command is skipped (rule 1) or its expansion is
pruned (rule 2), root#build still exists in the execution graph as a
passthrough. The edge `root#lint → root#build → successors` is preserved, so
lint runs before the other packages' builds.

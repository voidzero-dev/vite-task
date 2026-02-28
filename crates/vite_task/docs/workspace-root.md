# Workspace Root Handling

## RFC: Treat workspace root as a normal package

### Motivation

pnpm excludes the workspace root from recursive `run`/`exec` by default (since
v7) to prevent infinite loops — the classic case being a root script like
`"build": "pnpm -r run build"` that re-invokes itself. This works but introduces
special-case behavior, extra flags (`--workspace-root`, `--include-workspace-root`),
and user confusion about when the root is or isn't included.

vite-task can do better. Because `vp run` commands inside task scripts are
**parsed and expanded at plan time** (not spawned as child processes), the planner
has full visibility into the expansion. This means recursion can be detected and
cut structurally, without needing to exclude the root package from selection.

### Current behavior

- Root is always in the package graph; `-r` and `-t` include it like any other
  package.
- `vp run` commands inside task scripts are expanded at plan time (not spawned as
  child processes), giving the planner full visibility into nested expansions.
- Recursion is detected during planning — but treated as a fatal error rather
  than being skipped.

### The problem

```jsonc
// Workspace root package.json
{ "scripts": { "build": "vp run -r build" } }
```

Both of the following fail today:

```
$ vp run -r build
# ERROR — root#build's command is `vp run -r build`, the same command
#         that's already running → recursion error

$ vp run build      (from workspace root)
# ERROR — root#build's command `vp run -r build` expands to all packages,
#         including root#build itself → recursion error
```

The user's intent is clear: root's `build` should orchestrate building all
_other_ packages. The self-inclusion is incidental, not intentional.

### Proposal

**Do not special-case the workspace root.** Instead, handle recursion with two
rules:

**Rule 1 — Skip duplicate commands.** When a task's command is the **same
`vp run` invocation** that's already running, skip that command — it has nothing
new to contribute.

```jsonc
// Workspace root package.json
{ "scripts": { "build": "vp run -r build" } }
```

```
$ vp run -r build

# root#build's command `vp run -r build` is the same invocation already
# running → command is skipped. root#build becomes a passthrough (no work
# of its own). All other packages' build tasks run normally.
```

**Rule 2 — Prune self from nested expansions.** When a task's command expands
to a **different** `vp run` invocation whose results include the task itself,
remove the self-referential task from that expansion.

```jsonc
// Workspace root package.json
{ "scripts": { "build": "vp run -r build" } }
```

```
$ vp run build      (from workspace root)

# root#build's command `vp run -r build` is different from `vp run build`
# → expanded normally. The expansion includes root#build again → root#build
# is pruned from the expansion. Result: root#build orchestrates a#build
# and b#build nested inside it.
```

**Both rules together:**

| Scenario          | Rule                                          | Result                                   |
| ----------------- | --------------------------------------------- | ---------------------------------------- |
| `vp run -r build` | Rule 1 (same command)                         | root#build skipped, siblings run         |
| `vp run build`    | Rule 2 (different command, self in expansion) | root#build pruned from its own expansion |

Multi-command scripts work naturally — only the matching subcommand is skipped:

```jsonc
// Workspace root package.json — tsc runs, the recursive vp run is skipped
{ "scripts": { "build": "tsc && vp run -r build" } }
```

`dependsOn` edges through the passthrough node still work:

```jsonc
// root vite-task.json — root#lint runs first (dependsOn),
// root#build's own command is skipped, then other packages' build tasks run
{ "tasks": { "build": { "command": "vp run -r build", "dependsOn": ["lint"] } } }
```

Mutual recursion through different tasks is **not** handled — it remains an
error:

```jsonc
// Workspace root package.json
{
  "scripts": {
    "build": "vp run -r test",
    "test": "vp run -r build"
  }
}
```

```
$ vp run -r build

# root#build → root#test (different command, expanded normally)
#            → root#build (recursion, still a fatal error)
```

### Comparison with pnpm

| Aspect                  | pnpm                                           | vite-task (proposed)                                 |
| ----------------------- | ---------------------------------------------- | ---------------------------------------------------- |
| Root in `-r` by default | Excluded (since v7)                            | Included                                             |
| Recursion prevention    | Exclude root from selection                    | Skip duplicate commands + prune self from expansions |
| Scope                   | Only root package                              | Any task with self-referential commands              |
| Flags needed            | `--workspace-root`, `--include-workspace-root` | None                                                 |
| User model              | "root is special"                              | "all packages are equal"                             |

---

## Appendix: pnpm workspace root behavior (reference)

This section documents how pnpm treats the workspace root for comparison.

### Background

In a pnpm workspace, the root `package.json` typically serves as an
orchestrator — holding devDependencies for tooling and scripts for workspace
management — rather than being a publishable package. Including it in recursive
operations by default causes infinite loops — root scripts like
`"build": "pnpm -r run build"` create cycles when the root is included in its
own recursive invocation.
For this reason, pnpm excludes the workspace root from certain recursive
operations by default since v7.

### Affected commands

The automatic root exclusion applies only to `run` and `exec` when run
recursively (`-r`) with no explicit `--filter`.

All other recursive commands (e.g., `install`, `list`, `outdated`) include the
root as a normal workspace member.

### Filter logic

The implementation lives in `pnpm/src/main.ts`. When the `-r` (recursive) flag
is active:

1. User-provided `--filter` and `--filter-prod` values are collected.
2. If `--workspace-root` (`-w`) is set, an **inclusion** filter `{.}` is
   appended, adding the root to the selection.
3. Otherwise, if **all** of the following are true, an **exclusion** filter
   `!{.}` is appended, removing the root from the selection:
   - No user-provided filters exist (`filters.length === 0`).
   - A workspace directory is detected.
   - Workspace package patterns are configured.
   - `includeWorkspaceRoot` is not set.
   - The command is `run` or `exec`.

When the user provides explicit filters (e.g., `--filter .` or
`--filter root-pkg`), no implicit exclusion is applied — the user's intent takes
precedence.

### Flags

#### `--workspace-root` / `-w`

An **additive filter**. When used alongside `--filter`, it adds the workspace
root to the set of selected packages.

```
pnpm --filter my-pkg -w run build
# Runs build on both my-pkg AND the workspace root
```

#### `--include-workspace-root`

A **default override**. It disables the automatic exclusion of the workspace root
from unfiltered recursive commands.

```
pnpm -r --include-workspace-root run build
# Runs build on all workspace packages INCLUDING the root
```

Can be set persistently in `pnpm-workspace.yaml`:

```yaml
packages:
  - packages/*
includeWorkspaceRoot: true
```

### pnpm flag summary

| Flag                       | Short | Purpose                          | Used with         |
| -------------------------- | ----- | -------------------------------- | ----------------- |
| `--workspace-root`         | `-w`  | Add root to filter selection     | `--filter`        |
| `--include-workspace-root` | —     | Disable automatic root exclusion | `-r` (no filters) |

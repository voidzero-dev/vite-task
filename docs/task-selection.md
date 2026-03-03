# Task Selection

This document covers how to select which tasks to run: CLI flags (`-r`, `-t`, `--filter`), the `package#task` syntax, and pnpm filter compatibility.

## Basic Modes

### Single Package (default)

With no flags, `vp run` runs the task in the package that contains your current directory:

```bash
cd packages/app
vp run build        # runs build in @my/app only
```

### Specific Package (`package#task`)

Use the `package#task` syntax to target a specific package from anywhere:

```bash
vp run @my/app#build    # runs build in @my/app regardless of cwd
```

### Recursive (`-r`)

Run the task across **all** packages in the workspace, in topological (dependency) order:

```bash
vp run -r build     # builds every package, dependencies first
```

### Transitive (`-t`)

Run the task in the current package **and all its transitive dependencies**:

```bash
cd packages/app
vp run -t build     # builds @my/core, @my/lib, then @my/app
```

You can also combine `-t` with a package specifier:

```bash
vp run -t @my/app#build   # same as above, from any directory
```

### Workspace Root (`-w`)

Select the workspace root package explicitly:

```bash
vp run -w build     # runs build in the root package only
```

Can be combined with `--filter`:

```bash
vp run -w --filter @my/app build    # root + @my/app
```

## `--filter` Syntax

The `--filter` flag (short: `-F`) selects packages using patterns. The syntax is pnpm-compatible.

### By Package Name

```bash
# Exact name
vp run --filter @my/app build

# Glob pattern
vp run --filter "@my/*" build       # all packages under @my scope
vp run --filter "*utils*" build     # packages with "utils" in the name
```

### By Directory

```bash
# Exact directory
vp run --filter ./packages/app build

# With braces (equivalent)
vp run --filter {./packages/app} build

# Glob directory
vp run --filter "./packages/*" build     # all packages under packages/
```

### Name + Directory Intersection

Combine name and directory to narrow the selection:

```bash
vp run --filter "@my/app{./packages/app}" build
```

This selects packages matching BOTH the name pattern AND the directory.

### Graph Traversal

Append `...` to include transitive dependencies or dependents:

```bash
# Package + its transitive dependencies
vp run --filter "@my/app..." build

# Package + its transitive dependents
vp run --filter "...@my/core" build

# Dependencies only (exclude the package itself)
vp run --filter "@my/app^..." build

# Dependents only (exclude the package itself)
vp run --filter "...^@my/core" build

# Both directions
vp run --filter "...@my/lib..." build
```

**Example:** Given `core ← lib ← app`:

| Filter         | Selected packages   |
| -------------- | ------------------- |
| `@my/app...`   | app, lib, core      |
| `...@my/core`  | core, lib, app      |
| `@my/app^...`  | lib, core (not app) |
| `...^@my/core` | lib, app (not core) |

### Exclusion

Prefix with `!` to exclude packages:

```bash
# All packages except @my/utils
vp run --filter "@my/app..." --filter "!@my/utils" build
```

Exclusion filters are applied after all inclusion filters:

```
> vp run --filter "@my/app..." --filter "!@my/utils" build

# @my/app... selects: app, lib, core, utils
# !@my/utils removes: utils
# Final: app, lib, core
```

Execution plan:

```json
{
  "core#build": [],
  "lib#build": ["core#build"],
  "app#build": ["lib#build"]
}
```

### Multiple Filters (Union)

Multiple `--filter` flags produce a union:

```bash
vp run --filter @my/app --filter @my/cli build
```

This runs `build` in both `@my/app` and `@my/cli`.

### Space-Separated Filters

You can also pass multiple filters in a single value separated by spaces:

```bash
vp run --filter "@my/app @my/cli" build
```

### Auto-Completion of Scoped Names

If you use a bare name like `app` and there's no package named `app` but exactly one `@*/app` exists, Vite Task auto-resolves it:

```bash
vp run --filter app build    # resolves to @my/app (if unambiguous)
```

If multiple scoped packages match (e.g., `@scope-a/app` and `@scope-b/app`), Vite Task reports an ambiguity error.

### Unmatched Filter Warnings

When a filter doesn't match any package, Vite Task warns you:

```
> vp run --filter nonexistent build
WARN  No packages matched filter "nonexistent"
```

Exclusion-only filters that don't match anything do NOT produce warnings (since the intent to exclude is still valid).

## Topological Ordering with Filters

Filters with graph traversal (`...`) automatically enable topological ordering within the selected subgraph. Even when cherry-picking specific packages with `--filter`, the dependency order is respected:

```bash
vp run --filter @my/app --filter @my/core build
```

If `@my/app` depends on `@my/core` (transitively), `core#build` runs before `app#build`.

## pnpm Compatibility and Differences

The `--filter` syntax is designed to be pnpm-compatible. Most pnpm filter expressions work identically.

### What's the Same

| Feature                                      | pnpm | Vite Task |
| -------------------------------------------- | ---- | --------- |
| `--filter foo` (exact name)                  | ✓    | ✓         |
| `--filter "@scope/*"` (glob)                 | ✓    | ✓         |
| `--filter ./path` (directory)                | ✓    | ✓         |
| `--filter {./path}` (braced directory)       | ✓    | ✓         |
| `--filter foo...` (with deps)                | ✓    | ✓         |
| `--filter ...foo` (with dependents)          | ✓    | ✓         |
| `--filter "!foo"` (exclusion)                | ✓    | ✓         |
| `--filter foo^...` (deps only, exclude self) | ✓    | ✓         |
| Scoped name auto-completion                  | ✓    | ✓         |

### What's Different

**Workspace root handling\*:**

pnpm excludes the workspace root from `-r` / `--recursive` by default (since v7) to prevent infinite loops. It requires `--include-workspace-root` or `-w` to include it.

Vite Task includes the root like any other package. Recursion is prevented structurally by detecting and pruning self-referential `vp run` commands at plan time (see [Task Orchestration — Recursive Self-Reference](./task-orchestration.md#recursive-self-reference-handling)). This means:

| Aspect                  | pnpm                                           | Vite Task                              |
| ----------------------- | ---------------------------------------------- | -------------------------------------- |
| Root in `-r` by default | Excluded                                       | Included\*                             |
| Recursion prevention    | Exclude root from selection                    | Skip/prune self-referential commands\* |
| Extra flags needed      | `--workspace-root`, `--include-workspace-root` | None\*                                 |
| User model              | "root is special"                              | "all packages are equal"\*             |

**`--filter` cannot be combined with `-r` or `-t`:** `--filter` is a standalone selection mechanism. Combining it with `-r` or `-t` is an error — use `--filter "pkg..."` instead if you need transitive dependencies.

## CLI Flags Reference

| Flag                  | Short | Description                                          |
| --------------------- | ----- | ---------------------------------------------------- |
| `--recursive`         | `-r`  | Run in all packages, topological order               |
| `--transitive`        | `-t`  | Run in current package + its transitive dependencies |
| `--filter <pattern>`  | `-F`  | Select packages by name/directory/glob (repeatable)  |
| `--workspace-root`    | `-w`  | Select the workspace root package                    |
| `--ignore-depends-on` | —     | Skip explicit `dependsOn` dependencies               |
| `--verbose`           | `-v`  | Show full execution summary                          |
| `--last-details`      | —     | Show saved summary from last run                     |
| `--cache`\*           | —     | Force all caching on for this run                    |
| `--no-cache`\*        | —     | Force all caching off for this run                   |

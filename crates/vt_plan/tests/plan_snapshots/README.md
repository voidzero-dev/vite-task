# Plan Snapshot Tests

Tests for task graph construction and execution plan generation.

## When to add tests here

- Testing task graph structure (dependencies, task resolution)
- Testing execution plan generation from CLI arguments
- Testing task fingerprinting and cache key computation
- Testing workspace/package discovery and configuration parsing

## How it works

Each fixture in `fixtures/` is a self-contained workspace. Tests are defined in `snapshots.toml`:

```toml
[[plan]]
name = "descriptive test name"
args = ["build", "--recursive"]
cwd = "packages/app" # optional, defaults to workspace root
```

The test runner:

1. Copies the fixture to a temp directory
2. Loads the workspace and builds the task graph
3. Snapshots the task graph structure
4. For each plan test, parses CLI args and generates an execution plan
5. Compares against snapshots in `fixtures/<name>/snapshots/`

## Selecting plan snapshot fields

Set `fields` at the file level to keep each plan snapshot focused on the behavior
the fixture tests. For example, the `cache_keys` fixture retains task identities,
commands, and execution cache keys:

```toml
[fields]
key = true
neighbors = true
execution_item_display = { command = true }
cache_metadata = { execution_cache_key = true }

[[plan]]
name = "normal_task_with_extra_args"
args = ["run", "hello", "a.txt"]
```

The harness recursively searches for the keys directly under `[fields]`. Once a
key matches, nested selection tables match only direct children. `true` keeps the
entire value. For example, the selection above finds `cache_metadata` at any depth
and keeps its direct child `execution_cache_key` in full.

Arrays apply the selection to each element. Ancestors and array positions are
preserved; unmatched elements become `"<unselected>"` during recursive discovery.

Selected nulls, empty collections, and scalar values remain visible. For example,
`cache_metadata = { execution_cache_key = true }` preserves `cache_metadata: null`
when caching is disabled. `false` is rejected during deserialization. Empty
selection tables fail when reached during projection; selections for absent fields
are not visited. A selection that matches no outer fields also fails.

Omitting `fields`, or setting `fields = true` before any TOML table, keeps the full
plan. The option applies to non-compact plan cases in the file; it is not accepted
inside `[[plan]]`. Cases with `compact = true` keep their compact format and ignore
`fields`. Task graph Markdown and error snapshots are unaffected.

## Adding a new test

1. Create a new fixture directory under `fixtures/`
2. Add `package.json` (and `pnpm-workspace.yaml` for monorepos)
3. Add `snapshots.toml` with test cases (or omit for task-graph-only tests)
4. Run `cargo test -p vt_plan --test plan_snapshots`
5. Review and accept new snapshots

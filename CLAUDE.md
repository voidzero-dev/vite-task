# CLAUDE.md

## Project Overview

A monorepo task runner (like nx/turbo) with intelligent caching and dependency resolution.

## Build Commands

```bash
just init          # Install build tools and dependencies
just ready         # Full quality check (fmt, check, test, lint, doc)
just fmt           # Format code (cargo fmt, cargo shear, dprint)
just check         # Check compilation with all features
just test          # Run all tests
just lint          # Clippy linting
just doc           # Documentation generation
```

## Tests

```bash
cargo test                                              # All tests
cargo test -p vite_task_bin --test e2e_snapshots        # E2E snapshot tests
cargo test -p vite_task_plan --test plan_snapshots      # Plan snapshot tests
cargo test --test e2e_snapshots -- stdin                # Filter by test name
INSTA_UPDATE=always cargo test                          # Update snapshots
```

Integration tests (e2e, plan, fspy) require `pnpm install` in `packages/tools` first. You don't need `pnpm install` in test fixture directories.

Test fixtures and snapshots:

- **Plan**: `crates/vite_task_plan/tests/plan_snapshots/fixtures/` - quicker, sufficient for testing behaviour before actual execution:
  - task graph
  - resolved configurations
  - resolved program paths, cwd, and env vars
- **E2E**: `crates/vite_task_bin/tests/e2e_snapshots/fixtures/` - needed for testing execution and beyond: caching, output styling

## CLI Usage

```bash
# Run a task defined in vite-task.json
vp run <task>                        # run task in current package
vp run <package>#<task>              # run task in specific package
vp run <task> -r                     # run task in all packages (recursive)
vp run <task> -t                     # run task in current package + transitive deps
vp run <task> --extra --args         # pass extra args to the task command

# Built-in commands (run tools from node_modules/.bin)
vp test [args...]                    # run vitest
vp lint [args...]                    # run oxlint

# Flags
-r, --recursive                      # run across all packages
-t, --transitive                     # run in current package and its dependencies
--ignore-depends-on                  # skip explicit dependsOn dependencies
```

## Key Architecture

- **vite_task** - Main task runner with caching and session management
- **vite_task_bin** - CLI binary (`vp` command) and task synthesizer
- **vite_task_graph** - Task dependency graph construction and config loading
- **vite_task_plan** - Execution planning (resolves env vars, working dirs, commands)
- **vite_workspace** - Workspace detection and package dependency graph
- **fspy** - File system access tracking for precise cache invalidation

## Task Configuration

Tasks are defined in `vite-task.json`:

```json
{
  "tasks": {
    "test": {
      "command": "vitest run",
      "dependsOn": ["build", "lint"],
      "cache": true
    }
  }
}
```

## Task Dependencies

1. **Explicit**: Defined via `dependsOn` in `vite-task.json` (skip with `--ignore-depends-on`)
2. **Topological**: Based on package.json dependencies
   - With `-r/--recursive`: runs task across all packages in dependency order
   - With `-t/--transitive`: runs task in current package and its dependencies

## Code Constraints

These patterns are enforced by `.clippy.toml`:

| Instead of                          | Use                                      |
| ----------------------------------- | ---------------------------------------- |
| `HashMap`/`HashSet`                 | `FxHashMap`/`FxHashSet` from rustc-hash  |
| `std::path::Path`/`PathBuf`         | `vite_path::AbsolutePath`/`RelativePath` |
| `std::format!`                      | `vite_str::format!`                      |
| `String` (for small strings)        | `vite_str::Str`                          |
| `std::env::current_dir`             | `vite_path::current_dir`                 |
| `.to_lowercase()`/`.to_uppercase()` | `cow_utils` methods                      |

## Path Type System

- **Type Safety**: All paths use typed `vite_path` instead of `std::path`
  - **Absolute Paths**: `vite_path::AbsolutePath` / `AbsolutePathBuf`
  - **Relative Paths**: `vite_path::RelativePath` / `RelativePathBuf`

- **Usage Guidelines**:
  - Use `AbsolutePath` for internal data flow; only convert to `RelativePath` when saving to cache
  - Use methods like `strip_prefix`/`join` from `vite_path` instead of converting to std paths
  - Only convert to std paths when interfacing with std library functions
  - Add necessary methods in `vite_path` instead of falling back to std path types

## Quick Reference

- **Task Format**: `package#task` (e.g., `app#build`, `@test/utils#lint`)
- **Config File**: `vite-task.json` in each package

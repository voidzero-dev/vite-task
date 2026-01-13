# E2E Snapshot Tests

End-to-end tests that execute the `vite` binary and verify its output.

## When to add tests here

- Testing CLI behavior and output formatting
- Testing cache hit/miss behavior across multiple command invocations
- Testing error messages shown to users
- Testing integration between multiple commands in sequence

## How it works

Each fixture in `fixtures/` is a self-contained workspace. Tests are defined in `snapshots.toml`:

```toml
[[e2e]]
name = "descriptive test name"
steps = [
  "vite build",
  "vite build", # second run to test caching
]
```

The test runner:

1. Copies the fixture to a temp directory
2. Executes each step using `/bin/sh` (Unix) or `bash` (Windows)
3. Captures stdout/stderr and exit codes
4. Compares against snapshot in `fixtures/<name>/snapshots/`

## Adding a new test

1. Create a new fixture directory under `fixtures/`
2. Add `package.json` (and `pnpm-workspace.yaml` for monorepos)
3. Add `snapshots.toml` with test cases
4. Run `cargo test -p vite_task_bin --test e2e_snapshots`
5. Review and accept new snapshots

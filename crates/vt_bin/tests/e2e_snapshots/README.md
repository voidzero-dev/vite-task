# E2E Snapshot Tests

End-to-end tests that execute the `vt` binary and verify its output.

## When to add tests here

- Testing CLI behavior and output formatting
- Testing cache hit/miss behavior across multiple command invocations
- Testing error messages shown to users
- Testing integration between multiple commands in sequence

## How it works

Each fixture in `fixtures/` is a self-contained workspace. Tests are defined in `snapshots.toml`. A step is the argv of one command, as an array:

```toml
[[e2e]]
name = "descriptive_test_name"
steps = [
  ["vt", "run", "build"],
  ["vt", "run", "build"], # second run to test caching
]
```

Steps also support a table form with more options:

```toml
[[e2e]]
name = "interactive_step"
steps = [
  { argv = ["vt", "run", "dev"], comment = "cache miss", envs = [["MY_ENV", "1"]], interactions = [{ expect-milestone = "ready" }, { write = "hello" }, { write-line = "world" }, { write-key = "up" }, { write-key = "down" }, { write-key = "enter" }] },
  ["vtt", "pipe-stdin", "hello", "--", "vt", "run", "read-stdin"],
]
```

Notes:

- An array step is shorthand for `{ argv = [...] }`. A string step such as `"vt run build"` doesn't parse, and the test binary panics while loading the fixtures, before any case runs.
- A table step takes `argv` (required), `cwd` (relative to the fixture root, defaulting to the case's `cwd`), `comment` (shown under the step's heading in the snapshot), `envs` (`[name, value]` pairs), `interactions`, and `formatted-snapshot` (keep colors and styles in the snapshot). Other keys are an error.
- A case takes `name` (ASCII letters, digits, and `_` only; it also names the snapshot file), `steps`, and optionally `comment`, `cwd`, `cfg` (a Rust cfg expression such as `'target_os = "linux"'`; the case runs only where it matches), and `ignore` (the case runs only with `-- --ignored`).
- `vt` and `vtt` are the binaries built for the test. Other programs are looked up in `PATH`, with `packages/tools/node_modules/.bin` (when installed) ahead of the inherited entries.
- `write-key` accepts `up`, `down`, `enter`, `escape`, `backspace`, and `ctrl-c`.
- Snapshots include every interaction line, and each `expect-milestone` records the screen at that point.
- Steps don't run in a shell, so pipes, `&&`, and redirections aren't available. To pipe data to a command's stdin, use `vtt pipe-stdin <data> -- <command> [args...]`.

The test runner:

1. Copies the fixture to a temp directory
2. Starts each step's `argv` directly, without a shell, in an environment holding only `PATH`, `TERM`, and the step's `envs` (plus `PLAYWRIGHT_HOST_PLATFORM_OVERRIDE` when set, and a few system variables on Windows)
3. Runs each step in PTY mode (`TestTerminal`), with a timeout of 20 seconds (60 seconds on Windows and x86_64 macOS)
4. Applies configured interactions in order for PTY steps
5. Captures output and exit codes
6. Compares against the snapshot `fixtures/<fixture>/snapshots/<name>.md`

## Adding a new test

1. Create a new fixture directory under `fixtures/`
2. Add `package.json` (and `pnpm-workspace.yaml` for monorepos)
3. Add `snapshots.toml` with test cases
4. Run `cargo test -p vt_bin --test e2e_snapshots`
5. Review the new snapshots, written next to the expected ones as `<name>.md.new`, and accept them by re-running with `UPDATE_SNAPSHOTS=1`

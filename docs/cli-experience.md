# CLI Experience

This document covers the interactive terminal experience — what you see when running tasks, how output is displayed, and the various UI behaviors. For CLI flags controlling task selection (`-r`, `-t`, `--filter`, `-w`), see [Task Selection](./task-selection.md).

## Interactive Task Selector

When you run bare `vp run` (no task name) from within a package, an interactive task selector appears:

```
> vp run

Search task (↑/↓ to move, enter to select):
> build: echo build app
  lint: echo lint app
  test: echo test app
  lib#build: echo build lib
  lib#lint: echo lint lib
  lib#test: echo test lib
  lib#typecheck: echo typecheck lib
  root#check: echo check root
  root#clean: echo clean root
  root#deploy: echo deploy root
  root#docs: echo docs root
  root#format: echo format root
  (…3 more)
```

Tasks from the **current package** appear first (without the `#` prefix), followed by tasks from other packages in `package#task` format. Press **Enter** to select and run. Fuzzy search is supported (keyword `buid` matches task `build`).

### Non-Interactive Mode

When stdin is not a TTY (e.g., piped from another command), `vp run` prints the full task list to stdout instead:

```
> echo '' | vp run
  check: echo check root
  clean: echo clean root
  deploy: echo deploy root
  hello: echo hello from root
  app#build: echo build app
  app#lint: echo lint app
  app#test: echo test app
  lib#build: echo build lib
  lib#lint: echo lint lib
  lib#test: echo test lib
  lib#typecheck: echo typecheck lib
```

### Typo Correction ("Did You Mean")

When you specify a task that doesn't exist, Vite Task suggests alternatives:

**Interactive mode** — opens the selector with filtered results:

```
> vp run buid

Task "buid" not found.
Search task (↑/↓ to move, enter to select): buid
> app#build: echo build app
  lib#build: echo build lib
```

**Non-interactive mode** — prints suggestions:

```
> echo '' | vp run buid

Task "buid" not found. Did you mean:
  app#build: echo build app
  lib#build: echo build lib
```

## Task Output Display

### Single Task

When running a single task, output is displayed inline:

```
> vp run build
$ tsc
src/index.ts(3,1): error TS2304: Cannot find name 'foo'.
```

**Stdio inheritance:** For a single non-cached task, the spawned process inherits the terminal's stdin/stdout/stderr directly. This means:

- Interactive programs (prompts, progress bars) work correctly
- Colors are preserved (TTY detected)
- stdin is available for user input

```
> vp run dev
$ vite dev ⊘ cache disabled
stdin:tty
stdout:tty
stderr:tty
```

### Multiple Tasks

When running multiple tasks (e.g., with `-r`), each task's output is labeled with the working directory and command:

```
> vp run -r build

~/packages/core$ echo 'Building core'
Building core

~/packages/lib$ echo 'Building lib'
Building lib

~/packages/app$ echo 'Building app'
Building app
```

**Stdio piping:** With multiple concurrent tasks, output is captured (piped) rather than inherited. This means:

- Interactive programs won't receive stdin
- Output is grouped and displayed in order without interleaving
- Colors are still enabled via `FORCE_COLOR`

## Summary Output

After task execution, a summary line may appear depending on the scenario.

### Compact Summary (Default)

**Single task, cache miss:** No summary shown.

```
> vp run build
$ tsc
... output ...
```

**Single task, cache hit:** Thin line with duration saved.

```
> vp run build
$ tsc ✓ cache hit, replaying
... output ...

---
[vp run] cache hit, 1.5s saved.
```

**Multiple tasks, cache misses:**

```
> vp run -r build
~/packages/a$ print built-a
built-a

~/packages/b$ print built-b
built-b

---
[vp run] 0/2 cache hit (0%). (Run `vp run --last-details` for full details)
```

**Multiple tasks, mixed hits/misses:**

```
---
[vp run] 1/2 cache hit (50%), 234ms saved. (Run `vp run --last-details` for full details)
```

**Multiple tasks, failures:**

```
---
[vp run] 0/2 cache hit (0%), 2 failed. (Run `vp run --last-details` for full details)
```

### Verbose Summary (`-v` / `--verbose`)

The `-v` flag shows a detailed execution summary:

```
> vp run -r -v build
~/packages/a$ print built-a
built-a

~/packages/b$ print built-b
built-b


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] @my/a#build: ~/packages/a$ print built-a ✓
      → Cache miss: no previous cache entry found
  ·······················································
  [2] @my/b#build: ~/packages/b$ print built-b ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

With cache hits:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 2 cache hits • 0 cache misses
Performance:  100% cache hit rate, 1.2s saved in total

Task Details:
────────────────────────────────────────────────
  [1] @my/a#build: ~/packages/a$ print built-a ✓
      → Cache hit - output replayed - 650ms saved
  ·······················································
  [2] @my/b#build: ~/packages/b$ print built-b ✓
      → Cache hit - output replayed - 550ms saved
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

### Saved Summary (`--last-details`)

The verbose summary is persisted to disk after every run. You can recall it without re-running:

```bash
vp run --last-details
```

This reads and displays the saved summary from the last `vp run` invocation. The file is stored at `node_modules/.vite/task-cache/last-summary.json`.

If no previous run exists:

```
> vp run --last-details
error: No previous run details found
```

## Exit Codes

| Scenario                         | Exit code                         |
| -------------------------------- | --------------------------------- |
| All tasks succeed                | `0`                               |
| Single task fails                | The task's exit code (e.g., `42`) |
| Multiple tasks fail              | `1`                               |
| Task not found (non-interactive) | `1`                               |

```
> vp run -r fail
~/packages/a$ node -e "process.exit(42)"

~/packages/b$ node -e "process.exit(7)"

---
[vp run] 0/2 cache hit (0%), 2 failed.
```

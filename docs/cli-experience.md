# CLI Experience

This document covers the interactive terminal experience — what you see when running tasks, how output is displayed, and the various UI behaviors.

## Interactive Task Selector

When you run bare `vp run` (no task name) from within a package directory, an interactive task selector appears:

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

### Navigation

- **↑/↓** — move the cursor
- **Enter** — select the highlighted task and run it
- **Type text** — filter tasks by name (fuzzy search)
- **Escape** — clear the search query

### Search / Filtering

Typing narrows the list. For example, typing `lin`:

```
Search task (↑/↓ to move, enter to select): lin
> lint: echo lint app
  lib#lint: echo lint lib
```

### Smart Ranking

Tasks from the **current package** appear first (without the `#` prefix). Tasks from other packages appear below with the `package#task` format. When your search query contains `#`, this reordering is skipped — you get results sorted by relevance instead.

### Pagination

The selector shows up to 12 items at a time. If there are more, a `(…N more)` indicator appears at the bottom. Scrolling moves the visible window.

### After Selection

Once you select a task, it runs immediately:

```
Selected task: lint
~/packages/app$ echo lint app
lint app
```

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

**Important:** The interactive selector is only available for bare `vp run` (no flags like `-r`, `-t`, `-v`). When used with flags, a missing task is an error:

```
> vp run -r nonexistent
error: Task "nonexistent" not found in any package
```

### `vp run` in Scripts

`vp run` commands within task scripts also trigger the interactive selector if they don't specify a task:

```json
{
  "scripts": {
    "pick-task": "vp run"
  }
}
```

Running `vp run pick-task` shows the selector, and the selected task runs as a sub-task.

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
- Output is grouped and displayed in order
- Colors are still enabled via `FORCE_COLOR`

```
> vp run -r check-tty

~/packages/other$ check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

$ check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

### Cache Status Indicators

Each task line shows its cache status:

| Symbol                            | Meaning                                  | Styling        |
| --------------------------------- | ---------------------------------------- | -------------- |
| ✓ cache hit, replaying            | Output replayed from cache               | Green, dimmed  |
| ✗ cache miss: _reason_, executing | Cache invalidated                        | Purple, dimmed |
| ⊘ cache disabled                  | Task has `cache: false` or is a built-in | Gray           |
| _(nothing)_                       | First run, no previous cache             | Clean output   |

Examples:

```
$ tsc ✓ cache hit, replaying              # cached
$ tsc ✗ cache miss: args changed, executing  # changed
$ echo hello ⊘ cache disabled             # not cacheable
$ tsc                                      # first time
```

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

## Color Handling

- Vite Task respects the `NO_COLOR` environment variable (disables all color output)
- `FORCE_COLOR` is auto-detected based on terminal capability
- When spawning child processes, `FORCE_COLOR` is passed through so that tools like Jest, Vitest, and ESLint preserve their colored output even when piped

## Output Ordering

Tasks run in topological order (dependencies first). Within each dependency level, output appears in the order tasks complete. Stdout and stderr from each task are grouped together — you won't see interleaved output from different tasks.

When replaying cached output (cache hit), stdout and stderr chunks are replayed in the same chronological order they were originally captured, preserving the original output interleaving pattern within a single task.

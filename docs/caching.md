# Caching

Vite Task caches task execution results. When you re-run a task and nothing relevant has changed, the cached output is replayed instantly — the command never actually runs. This document explains how caching works internally, how cache validity is determined, and how to configure it.

## How It Works: Two-Level Fingerprinting

Caching uses a two-level fingerprint system:

1. **Spawn fingerprint** — computed _before_ execution. Captures the command, arguments, working directory, and environment variables.
2. **Post-run fingerprint** — computed _after_ execution. Captures the content hashes of all files the command actually read.

On the next run, the cache lookup works as follows:

```
Does the spawn fingerprint match?
│
├─ YES → Do input file hashes still match? (post-run fingerprint)
│        ├─ YES → CACHE HIT (replay output)
│        └─ NO  → CACHE MISS: "content of input 'foo.ts' changed"
│
└─ NO  → CACHE MISS: "args changed" / "envs changed" / etc.
```

Only **successful** task executions (exit code 0) are cached. Failed tasks are never cached.

## What Goes Into the Spawn Fingerprint

The spawn fingerprint is a composite of:

| Component               | Example                                     | Effect on cache                                |
| ----------------------- | ------------------------------------------- | ---------------------------------------------- |
| Command program         | `tsc`, `vp lint`                            | Program path change → miss                     |
| Arguments               | `["run", "--reporter", "verbose"]`          | Any arg change → miss                          |
| Working directory       | `packages/app` (relative to workspace root) | cwd change → miss                              |
| Fingerprinted envs      | `{ "NODE_ENV": "production" }`              | Value change → miss                            |
| Pass-through env config | `["CI", "PATH"]` (names only)               | Config change → miss (but value changes don't) |
| Fingerprint ignores     | `["dist/**", "*.tsbuildinfo"]`              | Pattern change → miss                          |

## What Goes Into the Post-Run Fingerprint

After a task runs successfully, Vite Task records which files the command read and their content hashes. On the next run (when the spawn fingerprint matches), these hashes are re-validated:

| Tracked item      | How it's checked                                |
| ----------------- | ----------------------------------------------- |
| File content      | xxHash3 content hash (fast, ~10GB/s)            |
| File existence    | File exists vs. doesn't exist                   |
| Directory entries | List of entries in directories that were listed |

If any tracked file has changed content, been added, or been deleted, it's a cache miss.

## File System Tracking with fspy

Vite Task doesn't require you to declare which files your command reads. Instead, it uses **fspy** (file system spy) to automatically track file access during execution.

When caching is enabled for a task, the spawned process's file system calls are intercepted and every file read is recorded. This happens transparently — the command runs normally, but Vite Task also captures what it touched.

**What fspy tracks:**

- File reads (opening a file for reading)
- Directory listings (readdir)
- Write access (noted but not used for cache invalidation)

**What fspy ignores:**

- File access outside the workspace directory
- Anything under `.git/`

This means the cache "just works" for most commands — `tsc` reads `.ts` files and `tsconfig.json`, so those become the cache inputs automatically. No configuration needed.

### When fspy Adds Too Much

fspy is intentionally cautious — it tracks _everything_ the command reads. Sometimes a command reads auxiliary files that you don't want to trigger cache invalidation (like `.tsbuildinfo` incremental files or build outputs that are also read during builds).

For these cases, you can use **negative patterns** in the inputs configuration or **fingerprint ignore patterns**.

## Task Inputs Configuration\*

The `inputs` field controls which files are tracked for cache invalidation. In most cases you don't need to configure this — fspy handles it automatically.

### Default Behavior (Auto-Inference)

When `inputs` is omitted, fspy auto-tracks everything the command reads:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        // inputs: not specified → fspy auto-tracks
      },
    },
  },
});
```

### Explicit Glob Patterns

Specify exactly which files to track (disables auto-inference):

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        inputs: ['src/**/*.ts', 'tsconfig.json'],
      },
    },
  },
});
```

Only files matching the globs are tracked. Files read by the command but not matching the globs are ignored.

### Auto-Inference with Exclusions

Track auto-inferred files but exclude certain patterns:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        inputs: [{ auto: true }, '!dist/**', '!**/*.tsbuildinfo'],
      },
    },
  },
});
```

Files in `dist/` and `.tsbuildinfo` files won't trigger cache invalidation even if the command reads them.

### Mixed Mode

Combine explicit globs with auto-inference:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        inputs: ['package.json', { auto: true }, '!**/*.test.ts'],
      },
    },
  },
});
```

- `package.json` is always tracked (explicit)
- Files read by the command are tracked (auto)
- Test files are excluded from both (negative pattern)

### No File Inputs

Disable all file tracking (cache only on command/env changes):

```ts
export default defineConfig({
  run: {
    tasks: {
      echo: {
        command: 'echo hello',
        inputs: [],
      },
    },
  },
});
```

### Summary Table

| Configuration                              | Auto-Inference | File Tracking                   |
| ------------------------------------------ | -------------- | ------------------------------- |
| `inputs` omitted                           | Enabled        | Inferred files                  |
| `inputs: [{ auto: true }]`                 | Enabled        | Inferred files                  |
| `inputs: ["src/**"]`                       | Disabled       | Matched files only              |
| `inputs: [{ auto: true }, "!dist/**"]`     | Enabled        | Inferred files except `dist/`   |
| `inputs: ["package.json", { auto: true }]` | Enabled        | `package.json` + inferred files |
| `inputs: []`                               | Disabled       | No files tracked                |

**Important:** Glob patterns are resolved relative to the **package directory** (where `package.json` lives), not the task's `cwd`.

## Environment Variables

### Fingerprinted Envs (`envs`)

These env vars are included in the cache fingerprint. If their value changes, the cache is invalidated:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        envs: ['NODE_ENV', 'VITE_*'],
      },
    },
  },
});
```

**Wildcard patterns\*:** `envs` supports glob-style wildcards:

- `NODE_*` — matches `NODE_ENV`, `NODE_PATH`, etc.
- `VITE_*` — matches all Vite environment variables
- `REACT_APP_*` — matches all Create React App variables

When an env value changes between runs:

```
> NODE_ENV=development vp run build
$ tsc
... output ...

> NODE_ENV=production vp run build
$ tsc ✗ cache miss: envs changed, executing
... output ...
```

### Pass-Through Envs (`passThroughEnvs`)

These env vars are passed to the process but are **not** part of the cache fingerprint. Changing their values does NOT invalidate the cache:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        passThroughEnvs: ['CI', 'GITHUB_ACTIONS'],
      },
    },
  },
});
```

The **names** of pass-through envs are part of the cache config — adding or removing a name from the list will invalidate the cache. But changing the _value_ of `CI` from `true` to `false` will not.

### Default Pass-Through Envs

Even without explicit configuration, a set of common environment variables are automatically passed through. These include:

- **System:** `HOME`, `USER`, `PATH`, `SHELL`, `LANG`, `TZ`, `TMP`, `TEMP`
- **Node.js:** `NODE_OPTIONS`, `COREPACK_HOME`, `PNPM_HOME`, `NPM_CONFIG_STORE_DIR`
- **CI/CD:** `CI`, `VERCEL_*`, `NEXT_*`
- **Terminal:** `TERM`, `COLORTERM`, `FORCE_COLOR`, `NO_COLOR`
- **IDEs:** `VSCODE_*`, `JB_IDE_*`
- **Docker:** `DOCKER_*`, `BUILDKIT_*`

### Sensitive Environment Variables\*

Environment variables matching sensitive patterns are automatically hashed with SHA-256 before being stored in the cache database. The plaintext value is never persisted. Sensitive patterns include:

- `*_KEY`, `*_SECRET`, `*_TOKEN`, `*_PASSWORD`
- `AWS_*`, `GITHUB_*`, `NPM_*TOKEN`
- `DATABASE_URL`, `MONGODB_URI`, `REDIS_URL`

These values are also automatically masked in console output: `API_KEY=***`.

### FORCE_COLOR Auto-Detection

Vite Task automatically sets `FORCE_COLOR` based on the terminal's color support level:

- `0` — no color
- `1` — basic ANSI (16 colors)
- `2` — 256 colors
- `3` — true color (16M colors)

This is applied unless `FORCE_COLOR` is already set or `NO_COLOR` is present.

## Cache Miss Reasons

When a cache miss occurs, Vite Task tells you exactly why. Here are all possible reasons:

### First Run (No Previous Cache)

```
$ tsc
... output ...
```

No inline message — the task runs normally.

### Command Changed

```
$ tsc --strict ✗ cache miss: args changed, executing
... output ...
```

### Environment Changed

```
$ tsc ✗ cache miss: envs changed, executing
```

Specifically detected changes include:

- `env added` — a new fingerprinted env was set
- `env removed` — a previously set fingerprinted env is now absent
- `env value changed` — a fingerprinted env's value differs

### Input File Changed

```
$ tsc ✗ cache miss: content of input 'src/index.ts' changed, executing
```

### Working Directory Changed

```
$ tsc ✗ cache miss: working directory changed, executing
```

### Pass-Through Env Config Changed

```
$ tsc ✗ cache miss: pass-through env added, executing
```

This happens when the `passThroughEnvs` list itself changes (names added/removed), not when values change.

## Shared Cache Entries

Two tasks with identical commands (same program, args, cwd, env config) share the same cache entry. This means if `@my/app#build` and `@my/lib#build` both run `tsc` with the same configuration, a cache hit for one can benefit the other — but only if they read the same files.

In practice, this is most useful for compound commands where sub-tasks share commands across different parent tasks.

## Cache Storage

Cache data is stored in a SQLite database at:

```
node_modules/.vite/task-cache/cache.db
```

This can be overridden with the `VITE_CACHE_PATH` environment variable.

The database uses WAL (Write-Ahead Logging) mode for safe concurrent access — multiple `vp` processes can read the cache simultaneously without corruption.

### Cache Initialization

The cache is **lazily initialized** — it's not loaded until the first cache lookup. This avoids SQLite race conditions when multiple `vp` processes start simultaneously, and avoids overhead for commands that don't need caching.

### Clearing the Cache

```bash
vp cache clean
```

This deletes the entire cache directory. All cache entries are lost and tasks will run fresh on the next invocation.

## Cache Hit/Miss Lifecycle

Here's the complete lifecycle of a cached task execution:

```
1. PLAN PHASE
   ├─ Parse task config
   ├─ Resolve environment variables
   ├─ Resolve working directory
   └─ Build spawn fingerprint

2. CACHE LOOKUP
   ├─ Look up by spawn fingerprint
   │   ├─ Found → validate post-run fingerprint (check input files)
   │   │   ├─ Valid   → CACHE HIT: replay stored stdout/stderr
   │   │   └─ Invalid → CACHE MISS: input file changed
   │   └─ Not found → check old execution key mapping
   │       ├─ Old mapping exists → CACHE MISS: spawn fingerprint mismatch
   │       └─ No mapping → CACHE MISS: no previous cache entry

3. EXECUTION (on cache miss)
   ├─ Spawn process with fspy tracking
   ├─ Capture stdout/stderr in real-time
   ├─ fspy records all file accesses
   └─ Process exits

4. CACHE UPDATE (only if exit code 0)
   ├─ Build post-run fingerprint (hash input files)
   ├─ Store spawn fingerprint → (outputs + post-run fingerprint)
   └─ Store execution key → spawn fingerprint
```

## Practical Examples

### Basic Cache Hit

```
> vp run build                                  # first run
$ tsc
... tsc output ...

> vp run build                                  # second run, nothing changed
$ tsc ✓ cache hit, replaying
... tsc output ...                              # replayed from cache

---
[vp run] cache hit, 1.5s saved.
```

### Compound Command with Partial Cache Hit

```json
{
  "scripts": {
    "task": "print foo && print bar"
  }
}
```

```
> vp run task                                   # first run
$ print foo
foo

$ print bar
bar

---
[vp run] 0/2 cache hit (0%).
```

Now change the first sub-command:

```json
{
  "scripts": {
    "task": "print baz && print bar"
  }
}
```

```
> vp run task                                   # second run
$ print baz ✗ cache miss: args changed, executing
baz

$ print bar ✓ cache hit, replaying
bar

---
[vp run] 1/2 cache hit (50%), <duration> saved.
```

The second sub-command (`print bar`) is still cached because its command didn't change.

### Cache Disabled

```ts
export default defineConfig({
  run: {
    tasks: {
      dev: {
        command: 'vite dev',
        cache: false,
      },
    },
  },
});
```

```
> vp run dev
$ vite dev ⊘ cache disabled
... dev server output ...

> vp run dev                                    # runs again, no caching
$ vite dev ⊘ cache disabled
... dev server output ...
```

### Input File Change

```
> vp run test                                   # first run
$ vitest run
... test output ...

# Edit src/utils.ts

> vp run test                                   # second run
$ vitest run ✗ cache miss: content of input 'src/utils.ts' changed, executing
... test output ...
```

### Environment Variable Change

```
> NODE_ENV=development vp run build             # first run with NODE_ENV=development
$ tsc
... output ...

> NODE_ENV=production vp run build              # NODE_ENV changed
$ tsc ✗ cache miss: envs changed, executing
... output ...
```

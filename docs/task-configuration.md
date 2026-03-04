# Task Configuration

Tasks are configured in the `run` section of your `vite.config.ts`. There are two ways tasks can exist: **explicit task definitions** and **package.json scripts**.

## Configuration Location

Each package can have its own `vite.config.ts` that configures tasks for that package:

```ts
// packages/app/vite.config.ts
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc',
        dependsOn: ['lint'],
        cache: true,
        envs: ['NODE_ENV'],
        passThroughEnvs: ['CI'],
      },
      lint: {
        command: 'vp lint',
      },
      deploy: {
        command: 'deploy-script --prod',
        cache: false,
      },
    },
  },
});
```

## Task Definition Schema

Each task supports these fields:

| Field             | Type       | Default             | Description                                                                                    |
| ----------------- | ---------- | ------------------- | ---------------------------------------------------------------------------------------------- |
| `command`         | `string`   | —                   | The shell command to run. If omitted, falls back to the package.json script of the same name.  |
| `cwd`             | `string`   | package root        | Working directory relative to the package root.                                                |
| `dependsOn`       | `string[]` | `[]`                | Explicit task dependencies. See [Task Orchestration](./task-orchestration.md).                 |
| `cache`           | `boolean`  | `true`              | Whether to cache this task's output.                                                           |
| `envs`            | `string[]` | `[]`                | Environment variables to include in the cache fingerprint.                                     |
| `passThroughEnvs` | `string[]` | (built-in defaults) | Environment variables passed to the process but NOT included in the cache fingerprint.         |
| `inputs`\*        | `Array`    | auto-inferred       | Which files to track for cache invalidation. See [Caching — Inputs](./caching.md#task-inputs). |

## Scripts vs Tasks

Vite Task recognizes two sources of runnable commands:

### 1. Package.json Scripts

Any `"scripts"` entry in a package's `package.json` is automatically available:

```json
// packages/app/package.json
{
  "name": "@my/app",
  "scripts": {
    "build": "tsc",
    "test": "vitest run",
    "dev": "vite dev"
  }
}
```

These scripts can be run directly with `vp run build` (from within the `packages/app` directory).

### 2. Explicit Task Definitions

Tasks defined in a package's `vite.config.ts` only affect that package. A task definition applies when:

- The package has a matching script in `package.json`, or
- The task itself specifies a `command`

```ts
// packages/app/vite.config.ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        // No command — uses this package's "build" script from package.json
        dependsOn: ['lint'],
        envs: ['NODE_ENV'],
      },
    },
  },
});
```

In this example, `build` runs `@my/app`'s own `package.json` `"build"` script but with the added `dependsOn` and cache configuration.

**Conflict rule:** If both the task definition and the `package.json` script specify a command, it's an error. Either define the command in `vite.config.ts` or in `package.json` — not both.

## Global Cache Configuration\*

The `cache` field is only allowed in the **workspace root** `vite.config.ts` and controls workspace-wide cache behavior:

```ts
// vite.config.ts (workspace root only)
export default defineConfig({
  run: {
    cache: { scripts: true, tasks: true },
  },
});
```

| Setting         | Type                            | Default                           | Meaning                                                                                               |
| --------------- | ------------------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `cache`         | `boolean \| { scripts, tasks }` | `{ scripts: false, tasks: true }` | Global cache toggle                                                                                   |
| `cache.tasks`   | `boolean`                       | `true`                            | When `true`, respects individual task cache config. When `false`, disables all task caching globally. |
| `cache.scripts` | `boolean`                       | `false`                           | When `true`, caches `package.json` scripts even without explicit task entries.                        |

Shorthands:

- `cache: true` → `{ scripts: true, tasks: true }`
- `cache: false` → `{ scripts: false, tasks: false }`

### CLI Overrides\*

You can override the global cache config per invocation:

```bash
vp run build --cache        # Force all caching on (scripts + tasks)
vp run build --no-cache     # Force all caching off
```

### When Is Caching Enabled?

A command run by `vp run` is either a **task** (has an entry in `vite.config.ts`) or a **script** (only exists in `package.json` with no corresponding task entry). A script that has a matching task entry is treated as a task.

```
--no-cache on the command line?
├─ YES → NOT CACHED (overrides everything)
└─ NO
   │
   --cache on the command line?
   ├─ YES → acts as cache: true (sets scripts: true, tasks: true)
   └─ NO  → uses workspace config
   │
   Does the command have a task entry in vite.config.ts?
   │
   ├─ YES (task) ──────────────────────────────────────────
   │   │
   │   Global cache.tasks enabled? (default: true, or true via --cache)
   │   ├─ NO  → NOT CACHED
   │   └─ YES
   │       │
   │       Per-task cache set to false?
   │       ├─ YES → NOT CACHED (--cache does NOT override this)
   │       └─ NO or not set → CACHED <----- this is the default for tasks
   │
   └─ NO (script) ─────────────────────────────────────────
       │
       Global cache.scripts enabled? (default: false, or true via --cache)
       ├─ YES → CACHED
       └─ NO  → NOT CACHED <----- this is the default for scripts
```

In short: **tasks are cached by default, scripts are not.** `--no-cache` turns off caching for everything. `--cache` is equivalent to `cache: true` — it enables both `cache.tasks` and `cache.scripts`, but cannot override a task's per-task `cache: false`.

## Compound Commands and Nested `vp run`

Commands joined with `&&` are split into independently-cached sub-tasks. Commands containing `vp run` calls are expanded at plan time into the execution graph. Both features work in task `command` fields and `package.json` scripts. See [Task Orchestration](./task-orchestration.md#compound-commands) for details.

## Environment Variables

See [Caching — Environment Variables](./caching.md#environment-variables) for full details on how `envs` and `passThroughEnvs` work with the cache system.

Quick summary:

- **`envs`**: Included in the cache fingerprint. Changing a value here invalidates the cache.
- **`passThroughEnvs`**: Passed to the process but NOT fingerprinted. Changing values here does NOT invalidate the cache.

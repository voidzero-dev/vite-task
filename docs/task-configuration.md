# Task Configuration

Tasks are configured in the `run` section of your `vite.config.ts`. There are two ways tasks can exist: **explicit task definitions** and **package.json scripts**.

## Configuration Location

```ts
// vite.config.ts (workspace root)
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    cache: { scripts: false, tasks: true }, // global cache settings*
    tasks: {
      build: {
        command: 'tsc',
        dependsOn: ['lint'],
        cache: true,
        envs: ['NODE_ENV'],
        passThroughEnvs: ['CI'],
      },
      lint: {
        command: 'eslint src',
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

Tasks defined in `vite.config.ts` are shared across all packages. A task definition applies to every package that has:

- A matching script in `package.json`, or
- The task itself specifies a `command`

```ts
// vite.config.ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        // No command — uses each package's own "build" script
        dependsOn: ['lint'],
        envs: ['NODE_ENV'],
      },
    },
  },
});
```

In this example, `build` will run each package's own `package.json` `"build"` script but with the added `dependsOn` and cache configuration from the task definition.

**Conflict rule:** If both the task definition and the `package.json` script specify a command, it's an error. Either define the command in `vite.config.ts` or in `package.json` — not both.

## Global Cache Configuration\*

The top-level `cache` field in the `run` config controls workspace-wide cache behavior:

```ts
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

## Compound Commands

Commands joined with `&&` are automatically split into independent sub-tasks, each cached separately:

```ts
export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc && rollup -c && terser dist/index.js',
      },
    },
  },
});
```

When you run `vp run build`, this becomes three cached sub-tasks. If you change only the terser config, `tsc` and `rollup` remain cached:

```
> vp run build
$ tsc ✓ cache hit, replaying
...
$ rollup -c ✓ cache hit, replaying
...
$ terser dist/index.js ✗ cache miss: content of input 'terser.config.js' changed, executing
...
---
[vp run] 2/3 cache hit (67%), 4.1s saved.
```

## Nested `vp run` in Scripts

Task scripts can contain `vp run` calls, which are **expanded at plan time** rather than spawned as child processes. This means the planner has full visibility into nested task execution and can optimize accordingly.

```json
// package.json
{
  "scripts": {
    "ci": "vp run lint && vp run test && vp run build"
  }
}
```

Running `vp run ci` expands all nested `vp run` calls into the execution plan. Each expanded task maintains its own caching. See [Task Orchestration — Nested Execution](./task-orchestration.md#nested-vp-run-expansion) for details.

## Environment Variables

See [Caching — Environment Variables](./caching.md#environment-variables) for full details on how `envs` and `passThroughEnvs` work with the cache system.

Quick summary:

- **`envs`**: Included in the cache fingerprint. Changing a value here invalidates the cache.
- **`passThroughEnvs`**: Passed to the process but NOT fingerprinted. Changing values here does NOT invalidate the cache.

# Vite Task — Overview

Vite Task is the monorepo task runner built into vite+ (the `vp run` command). It orchestrates scripts across your workspace packages. Think of it as an alternative to Turborepo or Nx — but integrated directly into your vite+ workflow and designed around pnpm workspaces.

**Key capabilities:**

- **Dependency-aware execution** — tasks run in the correct order based on your `package.json` dependency graph and explicit `dependsOn` declarations.
- **Intelligent caching** — task outputs are cached automatically. When nothing changes, tasks complete in milliseconds by replaying cached output.
- **File system tracking** — instead of manually declaring inputs, Vite Task monitors which files each command actually reads and uses that information to determine cache validity.
- **Compound command caching** — multi-command scripts like `tsc && rollup` are split into sub-tasks, each cached independently.
- **Familiar CLI** — if you've used pnpm, the package selection flags and workflow feel right at home.

## Documentation Map

| Document                                      | What it covers                                                                                  |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| [Task Configuration](./task-configuration.md) | How to define tasks, the config schema, scripts vs tasks, and cache options                     |
| [Task Orchestration](./task-orchestration.md) | Dependency resolution, execution order, compound commands, nested `vp run`                      |
| [Task Selection](./task-selection.md)         | CLI flags (`-r`, `-t`, `--filter`), pnpm compatibility, filter syntax                           |
| [Caching](./caching.md)                       | How caching works internally, fingerprinting, fspy, inputs configuration, environment variables |
| [CLI Experience](./cli-experience.md)         | Interactive task selector, terminal output, summary modes, error handling                       |

## Quick Example

Given a workspace:

```
my-app/
├── pnpm-workspace.yaml
├── package.json
├── packages/
│   ├── core/
│   │   ├── package.json        # @my/core
│   │   └── vite.config.ts
│   ├── lib/
│   │   ├── package.json        # @my/lib  →  depends on @my/core
│   │   └── vite.config.ts
│   └── app/
│       ├── package.json        # @my/app  →  depends on @my/lib
│       └── vite.config.ts
```

Each package has its own `vite.config.ts` that configures tasks for that package:

```ts
// packages/app/vite.config.ts
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    tasks: {
      build: {
        command: 'tsc && rollup -c',
        dependsOn: ['lint'],
      },
      lint: {
        command: 'vp lint',
      },
    },
  },
});
```

Running `vp run -r build` executes across all packages in dependency order:

```
> vp run -r build
~/packages/core$ vp lint
...
~/packages/core$ tsc && rollup -c
...
~/packages/lib$ vp lint
...
~/packages/lib$ tsc && rollup -c
...
~/packages/app$ vp lint
...
~/packages/app$ tsc && rollup -c
...
---
[vp run] 0/6 cache hit (0%). (Run `vp run --last-details` for full details)
```

Run it again — everything is cached:

```
> vp run -r build
~/packages/core$ vp lint ✓ cache hit, replaying
~/packages/core$ tsc && rollup -c ✓ cache hit, replaying
~/packages/lib$ vp lint ✓ cache hit, replaying
...
---
[vp run] 6/6 cache hit (100%), 3.2s saved in total
```

> **Note:** Features marked with \* are planned but not yet shipped.

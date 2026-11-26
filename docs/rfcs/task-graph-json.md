# RFC: Task graph as JSON (`vp run --json`)

## Summary

`vp run --json` with no task prints every package and task in the workspace, and how the tasks depend on each other, as one JSON document:

```bash
vp run --json
```

It is the JSON form of the task list `vp run` prints without a task, and like that list, it loads every package's `vite.config.*`. It runs nothing, and gives the same output from any directory in the workspace.

The companion RFC, [dry runs and complete miss reasons](./dry-run.md), covers `vp run --dry-run --json <task>`: what a particular run would do. It uses the conventions defined here.

## Motivation

- Shell completion, editor task panels, CI scripts and coding agents need the task list without parsing text or loading every `vite.config.*` ([vite-task#320](https://github.com/voidzero-dev/vite-task/pull/320)).
- Wiring checks need the `dependsOn` edges, which a flat list lacks ([vite-plus#2848](https://github.com/voidzero-dev/vite-plus/issues/2848)).
- "Which packages does `vp run` see?" needs the package list ([vite-plus#1182](https://github.com/voidzero-dev/vite-plus/issues/1182)).

## Example

A workspace with one library and one app that depends on it:

```
acme/
├── package.json            { "name": "acme", "private": true }
├── pnpm-workspace.yaml
├── packages/ui/
│   ├── package.json        { "name": "@acme/ui" }
│   └── vite.config.ts
└── apps/web/
    ├── package.json        { "name": "@acme/web", "dependencies": { "@acme/ui": "workspace:*" } }
    └── vite.config.ts
```

```ts
// packages/ui/vite.config.ts
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    tasks: {
      build: { command: 'tsdown' },
    },
  },
});
```

```ts
// apps/web/vite.config.ts
import { defineConfig } from 'vite-plus';

export default defineConfig({
  run: {
    tasks: {
      build: { command: 'vite build', cache: { env: ['NODE_ENV'] } },
      deploy: { command: 'wrangler deploy', cwd: 'infra', dependsOn: ['build'], cache: false },
    },
  },
});
```

`vp run --json` prints, in a terminal (captured output is the same on one line):

```json
{
  "$schema": "file:///tmp/vp_run-1000/task-graph_80cde1ad99307ef3eea7a658d7c95f88.json",
  "version": 1,
  "workspaceRoot": "/home/me/acme",
  "packages": {
    ".": { "name": "acme" },
    "apps/web": { "name": "@acme/web", "dependencies": ["packages/ui"] },
    "packages/ui": { "name": "@acme/ui" }
  },
  "tasks": {
    "apps/web#build": { "package": "apps/web", "name": "build", "command": "vite build" },
    "apps/web#deploy": {
      "package": "apps/web",
      "name": "deploy",
      "cwd": "apps/web/infra",
      "command": "wrangler deploy",
      "dependsOn": ["apps/web#build"]
    },
    "packages/ui#build": { "package": "packages/ui", "name": "build", "command": "tsdown" }
  }
}
```

Things to notice:

- A task's `package` is a key into `packages`, so its package name is `.packages[.package].name`.
- Only `deploy` has a `cwd`: its `cwd` setting moves it out of the package directory.
- `apps/web#build` has no `dependsOn`. That `-r` runs it after `packages/ui#build` comes from the package dependency, and the [run plan](./dry-run.md) shows it.

Typical uses:

```bash
# All task names, for completion
vp run --json | jq -r '.tasks[].name' | sort -u

# Does @acme/web have a deploy task?
vp run --json | jq -e '.tasks["apps/web#deploy"]' > /dev/null

# Every task as `name#task`, the form `vp run` accepts
vp run --json | jq -r '.packages as $p | .tasks[] | "\($p[.package].name // "")#\(.name)"'
```

## The document

```ts
type TaskGraph = {
  /**
   * `file://` URL of the JSON Schema that describes this document. Its descriptions are the
   * comments here. See `$schema` below.
   */
  $schema: string;

  /** Format version. It changes only when the format changes incompatibly; see Versioning below. */
  version: 1;

  /** Absolute path of the workspace root. Other paths are relative to it. */
  workspaceRoot: string;

  /** Every package in the workspace, keyed by path (`"."` for the root), even ones without tasks. */
  packages: {
    [path: string]: {
      /** The name in `package.json`. Absent when the package has none. */
      name?: string;

      /**
       * The other workspace packages listed in each `package.json` field, by path. Packages from
       * outside the workspace aren't listed. A package listed in two fields appears in both.
       * Absent when there are none.
       */
      dependencies?: string[];
      devDependencies?: string[];
      peerDependencies?: string[];
    };
  };

  /** Every task, keyed by task ID (see Task IDs below). */
  tasks: {
    [id: string]: {
      /** The path of the task's package, which is also its key in `packages`. */
      package: string;

      /** The task name, from `vite.config.*` or a `package.json` script. */
      name: string;

      /** The directory the command runs in, when the task's `cwd` setting changes it. */
      cwd?: string;

      /**
       * The command as the task defines it: a string, or an array of strings that run one after
       * another. A single command is always a string, even when the task defines it as a
       * one-element array.
       */
      command: string | string[];

      /**
       * The tasks in the task's own `dependsOn`, by ID. An entry such as
       * `{ task: 'build', from: 'dependencies' }` is replaced by the tasks it selects. Absent when
       * there are none.
       */
      dependsOn?: string[];
    };
  };
};
```

## Conventions

These apply to the run plan as well.

### Task IDs

An ID is `<package path>#<task name>`, e.g. `packages/ui#build`. A task of the root package is just its name, e.g. `ci`.

- In each part, `%` is written `%25` and `#` is written `%23`, so an ID has one literal `#`, or none for a root task, and two tasks never share an ID.
  - Without encoding `#`, task `c` in the package at `a#b` and task `b#c` in the package at `a` would both be `a#b#c`. Encoded, they are `a%23b#c` and `a#b%23c`.
  - Without encoding `%`, root tasks named `a#b` and `a%23b` would both be `a%23b`. Encoded, they are `a%23b` and `a%2523b`.
- Package names aren't part of IDs, since they can be absent or shared by two packages.
- Compare IDs; don't decode them. Every task also has `package` and `name`.

### Paths

A path inside the workspace is relative to `workspaceRoot`, with `/` on every platform; the workspace root itself is `"."`. A path outside it, such as a program the run plan starts, is absolute. `$schema` is a URL instead (below).

### `$schema`

`$schema` points to the document's JSON Schema (draft-07), so an agent can read what each field means, and an editor shows the descriptions and checks the document when it is opened as JSON. Each time it prints a document, vp writes the schema to a folder in the system temp directory that only the current user can write to, such as `/tmp/vp_run-1000`, unless it is already there. The file is named by a hash of its content, such as `task-graph_80cde1ad99307ef3eea7a658d7c95f88.json`, so documents printed by different vp versions point to their own schemas.

Its value uses a form that VS Code and JetBrains IDEs both read on every platform:

- `file://`, then the absolute path with `/` separators, with a `/` before a Windows drive: `file:///tmp/vp_run-1000/task-graph_80cde1ad99307ef3eea7a658d7c95f88.json`, `file:///C:/Users/me/AppData/Local/Temp/vp_run/task-graph_80cde1ad99307ef3eea7a658d7c95f88.json`.
- Not percent-encoded. JetBrains IDEs look the path up as written, so they wouldn't find a space written as `%20`; VS Code reads both.

A plain path wouldn't work on Windows: neither editor reads `C:\Users\…` as a schema location.

### Absent means empty or default

Empty lists and maps, and fields at their default value, are left out, so the document has no `null`, `[]` or `{}`. This saves tokens for AI agents, the main readers of the document. The tables above say what each absence means.

### Output

Indented when stdout is a terminal, one line otherwise. Fields appear in the order the types declare them; entries of a map keyed by path or ID, such as `packages` and `tasks`, are sorted by key. Nothing else goes to stdout: warnings go to stderr, and config files that print while loading must not reach stdout.

### Errors

When the document can't be produced, such as when a config can't be loaded, or, for a run plan, when the tasks it runs depend on each other in a cycle, the error goes to stderr as text, nothing is printed to stdout, and the exit code is non-zero. The task graph is printed even when `dependsOn` entries form a cycle.

### Versioning

`version` changes only for a breaking change: a rename, a removal, or a changed meaning. New fields and values don't change it; consumers ignore fields they don't know.

### Field names

camelCase, as in `vite.config.*`. Values that mirror `package.json` use its spelling (`devDependencies`), and `$schema` uses JSON Schema's.

## Flags

| Command                                        | Behaviour                                                                                                     |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `vp run --json`                                | The task graph. Implies non-interactive: no selector, even in a terminal.                                     |
| `vp run --json -r` (or `-t`, `-w`, `--filter`) | Error. The graph always covers the whole workspace; the [run plan](./dry-run.md) shows what a selection runs. |
| `vp run --json --dry-run`                      | Error: there is no task to preview.                                                                           |
| `vp run --json -v`                             | Error: there are no cache checks.                                                                             |
| `vp run --json --log=…`                        | Error: `--json` and `--log` conflict.                                                                         |

## Not included

- **Cache settings.** The [run plan](./dry-run.md) reports, per command, whether it is cached and what changed.
- **Package selection** (`--filter` and friends). The [run plan](./dry-run.md) shows what a selection runs.

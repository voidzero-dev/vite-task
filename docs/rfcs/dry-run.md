# RFC: Dry runs and complete miss reasons (`--dry-run`, `--json`, `-v`)

## Summary

Three flags for `vp run <task selector args>`, each doing one thing:

| Flag        | What it does                                                                                                                                                                    |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--dry-run` | Stops before running any command. Planning and cache checks happen, and print, as in a real run.                                                                                |
| `--json`    | Prints the answer as JSON instead of text. With a task, it requires `--dry-run` for now; without one, it prints the [task graph](./task-graph-json.md). Conflicts with `--log`. |
| `-v`, `-vv` | Checks every input instead of stopping at the first changed file. `-v` lists every change; `-vv` also lists everything the cached run depends on, and its outputs.              |

```bash
vp run --dry-run -r build             # what would run, and the first reason for each miss
vp run --dry-run -v -r build          # ... with every reason
vp run --dry-run --json -r build      # the same answer as JSON
vp run -v -r build                    # a real run whose summary lists every reason
```

The conventions (task IDs, paths, absent values, output, errors, versioning) are those of the [task-graph RFC](./task-graph-json.md#conventions).

## Motivation

- Once the work in [vite-task#929](https://github.com/voidzero-dev/vite-task/issues/929) lands, `vp run` caches tasks that read and rewrite their own files, such as Vitest, TypeScript and Astro, without exclusions in `cache.input` or `cache.output`. Whether such a task hits then depends on rules about which recorded files a hit may rebuild or find missing, not on config the user wrote. People and AI agents will need to see why a task missed, and what it was checked against, more than before.
- "Will this hit the cache?" can't be asked before running ([vite-task#791](https://github.com/voidzero-dev/vite-task/issues/791)).
- "What would this run do?" (which tasks, which commands, in what order) can't be asked either ([vite-plus#2848](https://github.com/voidzero-dev/vite-plus/issues/2848)).
- A miss reports only its first reason, so `'src/a.ts' modified` doesn't say whether reverting it is enough.

## The example

The workspace from the [task-graph RFC](./task-graph-json.md#example): `@acme/ui` (`packages/ui`, `build: tsdown`) and `@acme/web` (`apps/web`, depends on `@acme/ui`; `build: vite build` with `cache.env: ['NODE_ENV']`; `deploy: wrangler deploy`, `dependsOn: ['build']`, `cache: false`).

Yesterday `vp run -r build` ran both builds and cached them. Since then:

- `packages/ui/src/button.tsx` and `packages/ui/src/theme.css` were edited;
- the shell has `NODE_ENV=production`, where yesterday it had `development`.

## Each flag on its own

### `--dry-run`

`--dry-run` stops before running any command. Everything before that happens as in a real run and prints the same way: each command's line with its cache status, then the summary. There is no task output, and the status doesn't end in "executing" or "replaying".

```
$ vp run --dry-run -r build
~/packages/ui$ tsdown ○ cache miss: 'packages/ui/src/button.tsx' modified
~/apps/web$ vite build ○ cache miss: env 'NODE_ENV' changed

---
vp run: 0/2 cache hit (0%), nothing was run.
```

The real run prints the same lines, with the task output in between:

```
$ vp run -r build
~/packages/ui$ tsdown ○ cache miss: 'packages/ui/src/button.tsx' modified, executing
…

~/apps/web$ vite build ○ cache miss: env 'NODE_ENV' changed, executing
…

---
vp run: 0/2 cache hit (0%), 2/2 successful. (Run `vp run --last-details` for full details)
```

- Nothing runs and nothing is written: no process, no cache entry, no change to `vp run --last-details`.
- Everything else is a real run's: the plan, the cache check, every run flag and the args after the task name. The remote cache is read exactly when a real run would read it. Outputs are never downloaded or restored, since that would write files.
- Each step is checked against the files as they are now. In the JSON, a step that waits on one that would write files first, by running or by restoring outputs on a hit, is `unknown`: its files may change before it starts. The text shows today's check for it, and the real run below shows what can change.
- The first changed file is the first in path order, `cache.input` files before the files the task read.

### `-v`

`-v` keeps checking after the first changed file. Its detailed summary, in a dry run as in a real run, lists every change:

```
$ vp run --dry-run -v -r build
~/packages/ui$ tsdown ○ cache miss: 'packages/ui/src/button.tsx' modified
~/apps/web$ vite build ○ cache miss: env 'NODE_ENV' changed
…
Task Details:
────────────────────────────────────────────────
  [1] @acme/ui#build: ~/packages/ui$ tsdown
      → Cache miss: 'packages/ui/src/button.tsx' modified
                    'packages/ui/src/theme.css' modified
  ·······················································
  [2] @acme/web#build: ~/apps/web$ vite build
      → Cache miss: env 'NODE_ENV' changed
```

The list is complete: reverting every listed change would make the command a hit. Without `-v`, only the file check stops early; command, config and env changes are always listed in full.

In a real run:

```
$ vp run -v -r build
…
Task Details:
────────────────────────────────────────────────
  [1] @acme/ui#build: ~/packages/ui$ tsdown ✓
      → Cache miss: 'packages/ui/src/button.tsx' modified
                    'packages/ui/src/theme.css' modified
  ·······················································
  [2] @acme/web#build: ~/apps/web$ vite build ✓
      → Cache miss: env 'NODE_ENV' changed
                    'packages/ui/dist/index.js' modified
```

`packages/ui/dist/index.js` changed because `@acme/ui#build` rebuilt it first. The dry run, checking today's files, couldn't see that, which is why its JSON marks `@acme/web#build` as `unknown`.

### `-vv`

The detailed summary also lists what the check found unchanged, which shows, for an unexpected hit, whether an edited file is an input at all.

```
$ vp run --dry-run -vv --filter @acme/ui build
~/packages/ui$ tsdown ○ cache miss: 'packages/ui/src/button.tsx' modified
…
Task Details:
────────────────────────────────────────────────
  [1] @acme/ui#build: ~/packages/ui$ tsdown
      → Cache miss: 'packages/ui/src/button.tsx' modified
                    'packages/ui/src/theme.css' modified
        Unchanged: 'packages/ui/package.json'
                   'packages/ui/src' (listing)
                   'packages/ui/src/index.ts'
                   'packages/ui/tsdown.config.ts'
                   'packages/ui/.browserslistrc' (missing)
        Outputs:   'packages/ui/dist/index.d.ts'
                   'packages/ui/dist/index.js'
```

### `--json`

`--json` prints the same answer as a document. With `--dry-run`, that is the run plan:

```bash
vp run --dry-run --json -r build
```

```json
{
  "$schema": "https://viteplus.dev/schemas/run-plan.v1.json",
  "version": 1,
  "workspaceRoot": "/home/me/acme",
  "plan": {
    "apps/web#build": {
      "package": "apps/web",
      "name": "build",
      "dependsOn": ["packages/ui#build"],
      "steps": [
        {
          "program": "apps/web/node_modules/.bin/vite",
          "args": ["build"],
          "cache": {
            "status": "unknown",
            "baseline": { "durationMs": 9120 },
            "changes": [{ "kind": "env", "name": "NODE_ENV", "change": "changed" }]
          }
        }
      ]
    },
    "packages/ui#build": {
      "package": "packages/ui",
      "name": "build",
      "steps": [
        {
          "program": "packages/ui/node_modules/.bin/tsdown",
          "cache": {
            "status": "changed",
            "baseline": { "durationMs": 1830 },
            "changes": [
              { "kind": "file", "path": "packages/ui/src/button.tsx", "change": "changed" }
            ],
            "changesPartial": true
          }
        }
      ]
    }
  }
}
```

(As printed in a terminal; captured output is the same on one line.)

- `"changesPartial": true` marks a check that stopped at the first changed file; with `-v` it is gone.
- `@acme/web#build` is `unknown`: it waits on `@acme/ui#build`, which would run. Its `changes` are still today's check, and it has no `changesPartial` because that check found no changed file, so it checked them all.

## Flags together

`-v` and `-vv` change the same parts of the JSON as of the text. For `packages/ui#build`:

```jsonc
// --dry-run --json
"cache": {
  "status": "changed",
  "baseline": { "durationMs": 1830 },
  "changes": [{ "kind": "file", "path": "packages/ui/src/button.tsx", "change": "changed" }],
  "changesPartial": true
}

// --dry-run --json -v: complete, so no "changesPartial"
"cache": {
  "status": "changed",
  "baseline": { "durationMs": 1830 },
  "changes": [
    { "kind": "file", "path": "packages/ui/src/button.tsx", "change": "changed" },
    { "kind": "file", "path": "packages/ui/src/theme.css", "change": "changed" }
  ]
}

// --dry-run --json -vv: also everything the cached run depends on, changed or not, and its outputs
"cache": {
  "status": "changed",
  "baseline": {
    "durationMs": 1830,
    "fingerprint": {
      "paths": {
        "packages/ui/.browserslistrc": { "type": "missing" },
        "packages/ui/package.json": { "type": "file", "hash": "8f2c41d09a7be356" },
        "packages/ui/src": { "type": "dir", "entries": ["button.tsx", "index.ts", "theme.css"] },
        "packages/ui/src/button.tsx": { "type": "file", "hash": "1b9e07c4f26da813" },
        "packages/ui/src/index.ts": { "type": "file", "hash": "c3a5d8e2017f94b6" },
        "packages/ui/src/theme.css": { "type": "file", "hash": "5e0f9b72ad41c638" },
        "packages/ui/tsdown.config.ts": { "type": "file", "hash": "e7d24a6b9c05f183" }
      }
    },
    "outputs": ["packages/ui/dist/index.d.ts", "packages/ui/dist/index.js"]
  },
  "changes": [ … as with -v … ]
}
```

Every combination:

| Command                         | default                                                   | `-v`                                     | `-vv`                                            |
| ------------------------------- | --------------------------------------------------------- | ---------------------------------------- | ------------------------------------------------ |
| `vp run <sel>`                  | runs; one-line summary; each miss shows its first reason  | runs; detailed summary with every change | … and what it depends on                         |
| `vp run --dry-run <sel>`        | each command's cache status; one-line summary             | detailed summary with every change       | … and what it depends on                         |
| `vp run --dry-run --json <sel>` | the plan as JSON; `changesPartial: true` where it stopped | every change                             | … and `baseline.fingerprint`, `baseline.outputs` |
| `vp run --json <sel>`           | error: requires `--dry-run` (JSON lines later)            | the same                                 | the same                                         |

Two more combinations:

- **Structure only.** `vp run --dry-run --json --no-cache -r build` is what `vp run --no-cache` would do: every command is `{ "status": "disabled" }`, and no file is hashed and no remote request made.
- **Edges and disabled caching.** `vp run --dry-run @acme/web#deploy` shows `deploy` after `build`, through its `dependsOn`:

```json
"apps/web#deploy": {
  "package": "apps/web",
  "name": "deploy",
  "dependsOn": ["apps/web#build"],
  "steps": [
    {
      "program": "apps/web/node_modules/.bin/wrangler",
      "args": ["deploy"],
      "cwd": "apps/web/infra",
      "cache": { "status": "disabled" }
    }
  ]
}
```

### Rules

1. **A flag means the same thing in every combination.** `--dry-run` only stops execution, `--json` only changes the format, `-v`/`-vv` only change how much is checked and listed.
2. **Adding `--dry-run` to a working command line with a task never breaks it.** Flags that only shape execution (`--log`, `--concurrency-limit`) are accepted and do nothing. `vp run --json` without a task runs nothing, so it rejects `--dry-run`: there is no task to preview.
3. **A combination that isn't supported yet fails and says so**, like `--json` on a real run.
4. **vp's flags go before the task name.** Everything after it is passed to the task.

## The run plan

```ts
type RunPlan = {
  /**
   * The JSON Schema of this document, with the same descriptions as the comments here, so a
   * reader can look up what each field means. Its URL changes with `version`.
   */
  $schema: 'https://viteplus.dev/schemas/run-plan.v1.json';

  /** Format version. It changes only when the format changes incompatibly. */
  version: 1;

  /**
   * Absolute path of the workspace root. Other paths are relative to it, except paths outside
   * the workspace, which are absolute.
   */
  workspaceRoot: string;

  /**
   * The tasks this run would run, keyed by task ID. The IDs are the same as in `vp run --json`,
   * which has each task's package name and dependencies.
   */
  plan: {
    [id: string]: {
      /** The path of the task's package, such as `packages/ui`. */
      package: string;

      /** The task name, such as `build`. */
      name: string;

      /**
       * The tasks that must finish before this one starts, by ID: the task's own `dependsOn`,
       * plus, with `-r`, `-t` or `--filter`, the same task in the packages this package depends
       * on. A package without that task is skipped over. Absent when there are none.
       *
       * Each ID is a key of the same `plan` object as this task. A nested `plan` has its own
       * dependency graph: its tasks never depend on tasks of an enclosing or sibling plan, and
       * the same ID can appear in more than one plan.
       */
      dependsOn?: string[];

      /**
       * What the task runs, in order. Each part of the command between `&&`, each entry of a
       * command array, and each `pre`/`post` script is one step. Each step is cached separately.
       */
      steps: (
        | {
            /**
             * The executable that would be started, as `vp run` finds it: a path relative to
             * the workspace root if it is inside it (`apps/web/node_modules/.bin/vite`),
             * otherwise an absolute path (`/usr/local/bin/node`). A command with shell syntax,
             * such as a pipe, runs through the shell: `/bin/sh` with `["-c", "tsc | tee log"]`,
             * or `C:\Windows\System32\cmd.exe` with `/d /s /c` on Windows. For a built-in such
             * as `echo`, which runs without a new process, this is just its name.
             */
            program: string;

            /** The arguments, including any given after the task name. Absent when there are none. */
            args?: string[];

            /**
             * The directory it runs in, when the task's `cwd` setting or an earlier `cd` in the
             * command changes it. Absent when it runs in the package directory.
             */
            cwd?: string;

            /** Set when the step comes from a `pre` or `post` script, such as `prebuild`. */
            hook?: string;

            /**
             * Whether the cache would be used. Absent for a built-in such as `echo`.
             *
             * The check looks at the files as they are now. `unknown` marks a step whose files an
             * earlier step may change before it starts.
             */
            cache?:
              | {
                  /**
                   * - `hit`: matches `baseline`, so it would replay `baseline` instead of running.
                   * - `changed`: differs from `baseline`, so it would run. `changes` lists the differences.
                   * - `unknown`: can't be predicted yet, because a step it waits on would write files before
                   *   it starts, by running or by being a hit that restores outputs. A step waits on the
                   *   earlier steps of its task, on the tasks in its `dependsOn`, and, in a nested plan, on
                   *   the steps before the enclosing step. Only a step that would otherwise be `hit` or
                   *   `changed` can be `unknown`, and `changes` and `changesPartial` still show today's check.
                   */
                  status: 'hit' | 'changed' | 'unknown';

                  /**
                   * The cached run the step is compared with: the one with the same command,
                   * config and env, or, if there is none, the step's most recent cached run.
                   */
                  baseline: {
                    /** How long the cached run took, in milliseconds. On a hit, the time a replay saves. */
                    durationMs: number;

                    /**
                     * With `-vv`: everything the cached run depends on, changed or not: the env
                     * names and paths the cache records for this step.
                     */
                    fingerprint?: {
                      /** Env names. */
                      env?: string[];

                      /** Each path the cached run depends on, with what is checked. */
                      paths?: {
                        [path: string]:
                          | {
                              /** A file, matched by `cache.input` or read by the task. Its content is checked. */
                              type: 'file';

                              /**
                               * A hash of the content the cached run recorded. Use it only to compare with other
                               * hashes from the same version of Vite+: the algorithm isn't part of the format and
                               * can change, so don't compute hashes yourself to compare with it.
                               */
                              hash: string;

                              /**
                               * The task rewrites this file, so it may be missing, as on a fresh checkout. When it
                               * is there, its content is still checked.
                               */
                              mayBeMissing?: true;
                            }
                          | {
                              /** A directory the task opened. */
                              type: 'dir';

                              /**
                               * The names of the directory's entries, when the task listed it. Adding or removing an
                               * entry is a change. An empty array means the directory was empty. Absent when the
                               * task opened the directory without listing it: then only that it exists is checked.
                               */
                              entries?: string[];
                            }
                          | {
                              /** A path the task looked for and didn't find. Creating it is a change. */
                              type: 'missing';
                            };
                      };
                    };

                    /**
                     * With `-vv`: the files the cached run archived as its outputs, which a hit restores.
                     * Only files are listed: `packages/ui/dist/index.js` appears, but its directory
                     * `packages/ui/dist` doesn't get an entry of its own.
                     */
                    outputs?: string[];
                  };

                  /**
                   * Everything that differs from `baseline`, in the order the kinds are listed
                   * below, then by name or path.
                   * Undoing all of them would make the step a hit. Present for `changed`, and for
                   * `unknown` when today's check found differences. Absent for `hit`.
                   */
                  changes?: (
                    | {
                        /**
                         * The program as the cache records it: a path inside the workspace, or
                         * just the file name outside it, so `node` from another install is not
                         * a change.
                         */
                        kind: 'program';
                        from: string;
                        to: string;
                      }
                    | { kind: 'args'; from: string[]; to: string[] }
                    | { kind: 'cwd'; from: string; to: string }
                    | {
                        /**
                         * The setting changed. A changed `input` is one change: the files it
                         * now matches aren't listed.
                         */
                        kind: 'config';
                        setting: 'input' | 'output' | 'untrackedEnv';
                      }
                    | {
                        kind: 'env';
                        name: string;
                        change: 'added' | 'removed' | 'changed';
                      }
                    | {
                        kind: 'file';
                        path: string;
                        /** `added` also covers a new file in a directory the task listed. */
                        change: 'added' | 'removed' | 'changed';
                      }
                  )[];

                  /**
                   * Other files may have changed too: without `-v`, the file check stops at the
                   * first changed file. Command, config and env changes are always listed in
                   * full. Absent with `-v`.
                   */
                  changesPartial?: true;
                }
              | {
                  /**
                   * Would run: there is no cached run to compare with, as on a first run or after
                   * `vp cache clean`. Arguments given after the task name count, so a new
                   * combination of arguments is also `noBaseline`.
                   */
                  status: 'noBaseline';
                }
              | {
                  /**
                   * Would run: the cache couldn't be read. For example, the remote cache failed
                   * and nothing is cached locally.
                   */
                  status: 'error';

                  /** What went wrong. */
                  message: string;
                }
              | {
                  /** Would run without caching, because caching is off for this step. */
                  status: 'disabled';
                };
          }
        | {
            /**
             * The `vp run …` command this step stands for, as written, with any arguments given
             * after the task name: `vp run -r test`.
             */
            expandedFrom: string;

            /**
             * The plan of that nested `vp run`, in the same shape, so the plan is a tree. Its tasks
             * start when this step does, after the task's earlier steps, and their `dependsOn`
             * refers only to tasks in this nested plan. A nested run isn't started as a separate
             * process, so the step has no `program`, `cwd` or `cache`.
             */
            plan: RunPlan['plan'];
          }
      )[];
    };
  };
};
```

## Other behaviour

- **Placement.** Like every vp flag, `--dry-run` goes before the task name. `vp run deploy --dry-run` passes `--dry-run` to `deploy` and runs it.
- **Nested dry runs.** A task whose command runs `vp run --dry-run …` isn't inlined like other nested `vp run` commands: that command is started as a process, and it prints its own dry run.
- **Exit codes.** A dry run exits 0 when planning succeeds, whatever it predicts. When planning fails, the error goes to stderr and the exit code is non-zero.
- **`--last-details`.** After a run with `-v`, it shows every change.

## Changes to existing behaviour

- `-v` / `--verbose` becomes a counted flag: `-v`, `-vv` (or `--verbose --verbose`).
- `vp run -v` checks every input of a command that misses, and its detailed summary lists every change instead of the first.

## Future

- `--json` without `--dry-run` can print JSON lines as a real run progresses, one event per line.
- A hit that restores outputs makes every step that waits on it `unknown`, even when the restore rewrites identical files. If cache entries recorded each output's path and hash, a dry run could check those steps against the files the hit will restore, and only a restore that changes files a step reads would make it `unknown`.

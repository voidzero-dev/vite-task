# Task cache instability: findings and proposed fixes

Investigation of everything that makes a task unstable or uncacheable, run
against [emdash](https://github.com/emdash-cms/emdash) (20 packages, 25 build
tasks, mostly plain `tsdown`) with the automatic tracking rules from #571.

None of the proposals below are implemented or validated yet. Diagnoses are
marked **confirmed** (directly measured) or **hypothesis**.

**Environment.** `vp 0.0.0-commit.88ab4592d55342c11a070c47e8fc7205f675fda3`
(preview of voidzero-dev/vite-plus#2260, carrying #571), emdash branch
`agent/vite-plus-package-build-cache`. Local: macOS 26.5.1, M4 Pro, 12 cores.
CI: `ubuntu-latest`, 4 vCPU.

## Scope of the problem

Two things are worth separating, because they have very different weight.

**Repeat-run stability is already perfect.** Five consecutive warm runs with no
intervening change, all 25/25:

| run  | 1     | 2     | 3     | 4     | 5     |
| ---- | ----- | ----- | ----- | ----- | ----- |
| hits | 25/25 | 25/25 | 25/25 | 25/25 | 25/25 |

A no-op `pnpm install` also preserves 25/25 — it leaves
`node_modules/.pnpm-workspace-state-v1.json` byte-identical.

**Instability only appears across an environment transition** — a fresh checkout,
a real install, or a different machine. On CI, steady state is 22/25, and the same
three tasks miss every single run:

```
~/packages/registry-lexicons$ pnpm run build:lexicons ○ cache miss: 'node_modules/.pnpm-workspace-state-v1.json' modified
~/packages/registry-lexicons$ pnpm run build:types    ○ cache miss: 'node_modules/.pnpm-workspace-state-v1.json' modified
~/packages/core$ tsdown                               ○ cache miss: 'index.d.mts' removed from 'node_modules/emdash/dist'
```

Two distinct causes, both fixable in vite-task. Neither is a regression from #571;
cause A is a case #571's rules were meant to cover but attribute by the wrong
path.

|     | cause                                                                        | tasks | fixable in vite-task |
| --- | ---------------------------------------------------------------------------- | ----- | -------------------- |
| A   | workspace package reachable under two names via its `node_modules` self-link | 1     | yes                  |
| B   | package manager bookkeeping file read by any `pnpm run` task                 | 2     | yes                  |

---

## Cause A — a workspace package's output has two names

**Confirmed.** pnpm links every workspace package into `node_modules/<name>`, and
`packages/core` _is_ the package named `emdash`:

```
node_modules/emdash -> ../packages/core
```

So `packages/core/dist` and `node_modules/emdash/dist` are one directory. Tracing
`packages/core`'s own `tsdown` (119,037 accesses) shows it writing and reading the
same file under different names:

```
WRITE  packages/core/dist/index.d.mts          AccessMode(WRITE | CREATE | TRUNCATE)
READ   node_modules/emdash/dist/index.d.mts    AccessMode(READ)
```

The task consumes its own output because it resolves its own package through the
self-link. #571 has a rule for exactly this shape — a gitignored path underneath a
directory the task wrote into is the task's own derived state, not an input — and
`dist/` is gitignored at emdash's root. The rule does not fire because it compares
path strings: `node_modules/emdash/dist` is not lexically under `packages/core/`,
so the read is recorded as an input.

On a fresh checkout that path does not exist when the task starts, which is why
the miss says `removed from` rather than `modified`. The task can therefore never
hit on a clean machine.

**Blast radius.** Any workspace package whose build resolves its own package by
name. That is normal for packages with `exports` self-references, and it is not
emdash-specific.

### Proposed fixes

**A1 — canonicalise every tracked path (not recommended).** Resolve symlinks for
all accesses before classification. Simple rule, but 119k accesses for one task
means a `realpath` per path on a hot path that is currently pure string work. Worse,
pnpm's `node_modules/<dep>` entries point into `node_modules/.pnpm/...`, and under
some store layouts resolution can land outside the workspace root — where
`strip_prefix(workspace_root)` (`cache_update.rs:315`) silently drops the path, so
real dependency inputs would stop being tracked. Rejecting this because the failure
mode is a _lost_ input, which is worse than a missed cache.

**A2 — normalise workspace-package aliases using the package graph (recommended).**
vite-task already knows every workspace package's name and directory. Build a map
of `<name> -> <package dir>` once per run and rewrite any tracked path matching
`**/node_modules/<name>/...` to `<package dir>/...`. Pure string work, no syscalls,
and bounded by the number of workspace packages rather than the number of accesses.

After rewriting, core's read becomes `packages/core/dist/index.d.mts`, which _is_
under a directory it wrote into and _is_ gitignored, so the existing
own-derived-state rule fires unchanged and the task hits.

This also improves consumers: `packages/cloudflare`'s input becomes
`packages/core/dist/...` instead of `node_modules/emdash/dist/...`. Same file, but
the canonical name, so cache miss messages name the real path and two tasks
referring to one file agree on its identity.

Costs and open points:

- Must handle scoped names (`node_modules/@emdash-cms/plugin-cli`), which are two
  path segments, and nested `packages/x/node_modules/<name>`. Both were observed.
- `ClassifyContext` (`classify.rs:44`) currently has only `workspace_root`, the two
  infer flags and `is_gitignored`, so the map needs threading into `observe_fspy`
  (`cache_update.rs:194`). That is new plumbing but no new data source.
- Changes recorded input paths, so it needs a `CACHE_SCHEMA_VERSION` bump.

**A3 — narrow variant of A2, self-links only.** Rewrite only when the alias
resolves to _this task's own_ package directory. `SpawnFingerprint` already carries
`cwd: RelativePathBuf` (`cache_metadata.rs:90`, currently `pub(crate)`), so this
needs almost no plumbing — just widened visibility — and one `read_link` per
distinct alias.

Fixes the reported miss with the smallest possible behaviour change, and needs no
schema bump if consumers' paths are left alone. Does not give consumers the
canonical-name benefit.

**Recommendation: A2**, with A3 as the low-risk fallback if the plumbing or the
schema bump is unwelcome this cycle. A2 is a rule about identity that happens to
fix a caching bug; A3 patches the one symptom.

---

## Cause B — `pnpm run` makes a task uncacheable across machines

**Confirmed** that the file is tracked as an input to those two tasks and that its
content is unstable. **Hypothesis** that `pnpm` itself is the reader: tracing
`pnpm run build:lexicons` directly under fspy aborts with SIGABRT before it gets
that far, which is the pre-existing pnpm SEA bug (`Assertion failed: (magic) ==
(kMagic)`) recorded against `main`. The attribution rests on vite-task naming the
file as the changed input for exactly the two tasks that spawn pnpm, and on no
other emdash task touching it.

`registry-lexicons` is the only package whose build wraps the package manager:

```json
"build": "pnpm run build:lexicons && pnpm run build:types"
```

That file cannot be fingerprinted stably, by construction:

```
lastValidatedTimestamp: 1785115273801            <- wall clock
projects: { "/Users/chwang/code/emdash": {...},  <- absolute paths
            "/Users/chwang/code/emdash/apps/aggregator": {...}, ... }
```

The timestamp changes on any revalidating install. The absolute paths differ
between machines and between a local checkout and `/home/runner/work/emdash/emdash`,
so this **also defeats cache portability across workspace roots**, which vite-task
otherwise supports and tests.

It is not caught by #571's always-ignored set. That set (`node_modules`, `.git`)
only resolves read-then-write overlaps; this is a pure read, and pure reads under
`node_modules` are legitimate inputs — that is how dependency code is tracked.

**Blast radius.** Every task in every pnpm workspace that wraps a script in
`pnpm run`, which is a very common pattern. Each such task is permanently
uncacheable on CI. This is the more widely damaging of the two causes even though
it looks like the more niche one.

### Proposed fixes

**B1 — ignore package-manager bookkeeping files as inputs (recommended).** Keep a
small known list of files that are a package manager's own state and never an
authored source or a meaningful dependency. Present in the emdash checkout and
verified to exist:

```
node_modules/.pnpm-workspace-state-v1.json   <- the one causing the miss
node_modules/.modules.yaml
node_modules/.pnpm/lock.yaml
```

Equivalents for other managers, named from their documented layouts and not
observed here, so worth confirming before adding:

```
node_modules/.package-lock.json   (npm)
node_modules/.yarn-state.yml      (yarn, node_modules linker)
```

Applied to reads, not just overlaps. Low risk: these files describe how
`node_modules` was produced, and the thing that actually matters — the installed
package contents — is already tracked directly.

`node_modules/.package-map.json` also exists in emdash but is a weaker candidate:
it contains no absolute paths, so it does not break portability the way the
workspace-state file does. Leave it out until something is shown to be destabilised
by it.

Worth pairing with a rule of thumb for the list: a file belongs on it only if its
content is derived from the lockfile plus the local environment, so that anything
it could tell a task is either already tracked or is machine identity.

**B2 — ignore by shape rather than by name.** Treat `node_modules/.<anything>` as
manager state. Simpler and needs no list maintenance, but over-broad:
`node_modules/.bin/*` lives there and is a real input (three `.bin` reads appear in
core's trace), and `node_modules/.vite/` is vite's own cache directory.

**B3 — leave it to users.** Document a manual `input` exclusion, which is what
[When To Add Manual Config](https://viteplus.dev/guide/automatic-data-tracking#when-to-add-manual-config)
already implies. Zero risk, but every pnpm monorepo has to rediscover this, and the
symptom — one task that never hits, only on CI — is expensive to diagnose. The
whole point of automatic tracking is not needing this.

**Recommendation: B1.** B2's over-reach is a real regression risk for `.bin`, and
B3 pushes a general defect onto every user.

On the emdash side there is also a one-line workaround worth noting: the package's
own `prepublishOnly` already uses `node --run build`, and switching `build` to
`node --run` avoids spawning pnpm at all. That is a good local fix but does not
help the general case.

---

## Not a cause: `--parallel`

I previously recorded `--parallel` losing cache hits (18/25, reproducible 3/3) as a
possible correctness bug. **That was wrong, and this corrects it.** `vp run --help`
is explicit:

```
--parallel   Run tasks without dependency ordering. Sets concurrency to
             unlimited unless `--concurrency-limit` is also specified
```

Timestamped task starts confirm the documented behaviour rather than a bug:

|                   | `packages/core` starts | consumers start                              |
| ----------------- | ---------------------- | -------------------------------------------- |
| default limit (4) | 444.832                | 470.767 – 475.772 (after core finishes)      |
| `--parallel`      | 382.231                | 382.144 – 382.466 (cloudflare _before_ core) |

With ordering discarded, consumers fingerprint `node_modules/emdash/dist` while
core is still writing it, so the cold run stores entries describing a partially
written directory and the warm run correctly reports `added in`. Using `--parallel`
on a workspace where packages consume each other's build output is simply the wrong
flag. Nothing to fix.

One optional hardening, low priority: when a task's inferred inputs overlap another
task's inferred outputs _within the same run_, the entry is built on unordered
state. vite-task could decline to cache that task, or warn. It would convert a
silently useless cache entry into a visible explanation. Worth it only if
`--parallel` misuse turns out to be common.

## Not a cause: paths outside the workspace root

`strip_prefix(workspace_root)` (`cache_update.rs:315`) drops every access outside
the workspace, so the 388 `/var/folders` temp accesses and the `/opt/homebrew`
reads in core's trace cannot destabilise anything.

The same rule means the toolchain is not fingerprinted: `node` lives under
`~/.local/share/mise/...`, so a Node or `tsdown` upgrade does not invalidate a
cached task. That is a deliberate portability tradeoff, not instability, but it is
worth being explicit that cache keys describe the workspace and not the machine.

## Checked and clean

For `packages/core`'s build, none of the usual suspects appear: no `.tsbuildinfo`,
no `node_modules/.vite/` cache reads, no `pnpm-lock.yaml` read, no log files. The
only `node_modules/.bin` accesses are three plain reads of shims. It also touches
none of the manager state files from cause B — zero accesses to
`.pnpm-workspace-state-v1.json`, `.modules.yaml`, `.package-map.json`,
`.pnpm/lock.yaml`, `.vite/` or `.vite-temp/` — which is consistent with only the
two pnpm-spawning tasks missing.

`node_modules/.vite-temp/` deserves a note because it looks dangerous and is not:
vitest writes `vitest.config.js.timestamp-<n>.mjs` files there, so its directory
listing changes every run. #571 classifies those as write-first outputs and the
directory is empty at archive time, and the e2e fixture covering it passes. It is
not implicated in emdash's builds.

So for this workspace the instability really is just causes A and B, not a long
tail.

That is a statement about emdash, not a general audit. A workspace using
`tsc --build` would add `.tsbuildinfo`, which embeds absolute paths and timestamps
and would likely need the same treatment as cause B.

## Suggested order of work

1. **B1** — smallest change, widest benefit, affects every pnpm monorepo.
2. **A2** — larger, needs plumbing and a schema bump, but fixes a real gap in
   #571's own rule and makes path identity consistent.
3. Optional `--parallel` diagnostic, only if misuse is seen in the wild.

Both A and B should be reproduced as e2e fixtures first, since both are currently
only observed through emdash. A fixture for A needs a package whose build resolves
its own name through a `node_modules` self-link; for B, a task that shells out to
`pnpm run` with a workspace state file present.

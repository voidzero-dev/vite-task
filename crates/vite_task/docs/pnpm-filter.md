# Specification: `pnpm run --filter` / `--filter-prod` with Exact and Glob Task Names

## Context

This document describes the end-to-end behavior of `pnpm run` when combined with `--filter` or `--filter-prod` (also written `--prod-filter`). The flow is split into two independent stages: **package selection** (which workspace packages to consider) and **task matching** (which scripts to run within each selected package).

This spec is based on pnpm@dcd16c7b36cf95dc2abb9b09a81d66e87cd3fe97.

---

## Stage 1: Package Selection

### 1.1 CLI Parsing

When the CLI receives `--filter <pattern>` or `--filter-prod <pattern>`:

1. [parse-cli-args](cli/parse-cli-args/src/index.ts) detects the `--filter` / `--filter-prod` option via `nopt`. If either is present, the command is implicitly made **recursive** (`options.recursive = true`).

2. [config](config/config/src/index.ts) normalizes the option: if the value is a single string, it is **split by spaces** into an array. So `--filter "a b"` becomes `["a", "b"]`.

3. [main.ts](pnpm/src/main.ts:200-203) wraps each filter string into a `WorkspaceFilter` object:
   - Strings from `--filter` get `{ filter: <string>, followProdDepsOnly: false }`.
   - Strings from `--filter-prod` get `{ filter: <string>, followProdDepsOnly: true }`.

### 1.2 Selector Parsing

Each filter string is parsed by [`parsePackageSelector`](workspace/filter-workspace-packages/src/parsePackageSelector.ts) into a `PackageSelector`:

```
interface PackageSelector {
  namePattern?: string       // e.g. "a", "@scope/pkg", "foo*"
  parentDir?: string         // e.g. resolved absolute path from "./packages"
  diff?: string              // e.g. "master" from "[master]"
  exclude?: boolean          // leading "!" in the original string
  excludeSelf?: boolean      // "^" modifier
  includeDependencies?: boolean  // trailing "..."
  includeDependents?: boolean    // leading "..."
  followProdDepsOnly?: boolean   // from --filter-prod
}
```

Parsing rules (applied in order on the raw string):

1. Leading `!` → set `exclude: true`, strip.
2. Trailing `...` → set `includeDependencies: true`, strip. Then trailing `^` → set `excludeSelf: true`, strip.
3. Leading `...` → set `includeDependents: true`, strip. Then leading `^` → set `excludeSelf: true`, strip.
4. Remainder is matched against regex: `name{dir}[diff]` extracting the three optional parts.
5. If the regex doesn't match and the string looks like a relative path (starts with `.` or `..`), it becomes `parentDir`.

### 1.3 Building the Dependency Graph

[`filterPkgsBySelectorObjects`](workspace/filter-workspace-packages/src/index.ts:90-147) splits selectors into two groups:

- **prod selectors** (`followProdDepsOnly: true`)
- **all selectors** (`followProdDepsOnly: false`)

For each group, a **separate package graph** is built via [`createPkgGraph`](workspace/pkgs-graph/src/index.ts):

- The **all** graph includes: `dependencies`, `devDependencies`, `optionalDependencies`, `peerDependencies`.
- The **prod** graph includes: `dependencies`, `optionalDependencies`, `peerDependencies` (i.e. `devDependencies` are excluded via `ignoreDevDeps: true`).

Each graph is a map from `ProjectRootDir` → `{ package, dependencies: ProjectRootDir[] }`. A dependency edge exists only when it resolves to another workspace package (via version/range matching or `workspace:` protocol).

### 1.4 Selecting Packages from the Graph

[`filterWorkspacePackages`](workspace/filter-workspace-packages/src/index.ts:149-178) applies the selectors to the graph:

1. Selectors are partitioned into **include** selectors and **exclude** selectors (those with `exclude: true`).
2. If there are no include selectors, **all** packages in the graph are initially selected.
3. For each include selector, `_filterGraph` is called.
4. For exclude selectors, `_filterGraph` is called similarly.
5. Final result = `include.selected − exclude.selected`.

### 1.5 `_filterGraph` — The Core Selection Algorithm

[`_filterGraph`](workspace/filter-workspace-packages/src/index.ts:180-273) maintains three sets and one list:

- `cherryPickedPackages` — directly matched, no graph traversal
- `walkedDependencies` — collected by walking dependency edges
- `walkedDependents` — collected by walking reverse-dependency edges
- `walkedDependentsDependencies` — dependencies of walked dependents

For each selector:

**Step A — Find entry packages:**

- If `namePattern` is set: match package names using [`createMatcher`](config/matcher/src/index.ts) (a glob matcher that converts `*` to `.*` regex; no `*` means exact match).
  - Bonus: if a non-scoped pattern yields zero matches, it retries as `@*/<pattern>` and accepts the result only if exactly one package matches.
- If `parentDir` is set: match packages whose root dir is under that path.

**Step B — Expand via graph traversal (`selectEntries`):**

- If `includeDependencies` is true: walk the dependency graph forward from entry packages (DFS), adding all reachable nodes to `walkedDependencies`. If `excludeSelf` is true, the entry package itself is not added (but its dependencies still are).
- If `includeDependents` is true: walk the **reversed** graph from entry packages, adding all reachable nodes to `walkedDependents`. Same `excludeSelf` logic.
- If **both** `includeDependencies` and `includeDependents`: additionally walk forward from all walked dependents into `walkedDependentsDependencies`.
- If **neither**: simply push entry packages into `cherryPickedPackages`.

**Step C — Combine:** The final selected set is the union of all four collections.

### 1.6 Merging Results

If both `--filter` and `--filter-prod` selectors exist, their selected graphs are merged. The merge is ([filterPkgsBySelectorObjects](workspace/filter-workspace-packages/src/index.ts#L132-L137)):

```
selectedProjectsGraph = { ...prodFilteredGraph, ...filteredGraph }
```

This is a JS object spread, so for any package that appears in **both** graphs, the node from `filteredGraph` (full graph, with devDep edges) **overwrites** the node from `prodFilteredGraph` (prod graph, without devDep edges). This creates an asymmetry: the **graph origin of each node** determines which dependency edges it carries into the final `selectedProjectsGraph`. See Examples 6-8 for the implications.

---

## Stage 2: Task Matching and Execution

Package selection (Stage 1) is completely independent of task names. It doesn't look at `scripts` at all. Task matching happens later, within the `run` command.

### 2.1 Entry to `runRecursive`

The [`run` handler](exec/plugin-commands-script-runners/src/run.ts:189-305) checks `opts.recursive`. If true and there's a script name (or more than one selected package), it delegates to [`runRecursive`](exec/plugin-commands-script-runners/src/runRecursive.ts:43-201), passing the full `selectedProjectsGraph`.

### 2.2 Topological Sorting

If `opts.sort` is true (the default), packages in `selectedProjectsGraph` are topologically sorted via [`sortPackages`](workspace/sort-packages/src/index.ts:18-21). The sort only considers edges **within the selected graph** (edges to non-selected packages are ignored). The result is an array of "chunks" — each chunk is a group of packages with no inter-dependencies that can run in parallel.

### 2.3 Per-Package Script Matching

For each package in each chunk, [`getSpecifiedScripts`](exec/plugin-commands-script-runners/src/runRecursive.ts:217-232) determines which scripts to run:

1. **Exact match first:** If `scripts[scriptName]` exists, return `[scriptName]`.
2. **Regex match:** If the script name has the form `/pattern/` (a regex literal), [`tryBuildRegExpFromCommand`](exec/plugin-commands-script-runners/src/regexpCommand.ts) extracts the pattern and builds a `RegExp`. All script keys in the package's `scripts` that match this regex are returned. Regex flags (e.g. `/pattern/i`) are **not supported** and throw an error.
3. **No match:** Return `[]`.

Note: this is **not** glob matching. Task name patterns use **regex literal syntax** (`/pattern/`), while package name patterns in `--filter` use **glob syntax** (`*`). They are different systems.

### 2.4 Handling Packages Without Matching Scripts

When `getSpecifiedScripts` returns an empty array for a package:

- The package's result status is set to `'skipped'`.
- The package is simply not executed — no error is raised for that individual package.

### 2.5 Error Conditions

After iterating through all packages:

- If **zero** packages had a matching script (`hasCommand === 0`), **and** the script name is not `"test"`, **and** `--if-present` was not passed:
  - Error: `RECURSIVE_RUN_NO_SCRIPT` — "None of the selected packages has a `<scriptName>` script."
- Additionally, if `requiredScripts` config includes the script name, **all** selected packages must have it, or an error is thrown before execution begins.

### 2.6 Execution Order

The topological sort and chunking operate at the **package** level, not the individual script level. The execution loop ([runRecursive.ts:90-183](exec/plugin-commands-script-runners/src/runRecursive.ts#L90-L183)) processes one chunk at a time:

1. For each chunk, collect all `(package, scriptName)` pairs by running `getSpecifiedScripts` on every package in the chunk, then flatten.
2. Run all collected scripts in the chunk **concurrently** (up to `workspaceConcurrency` limit) via `Promise.all`.
3. **Await** the entire chunk before proceeding to the next.

This means: if package b is in chunk N and package a is in chunk N+1, **all** of b's matched scripts finish before **any** of a's matched scripts start — regardless of whether the matched script names are the same or different. Within a single chunk, scripts from different packages (and even multiple matched scripts from the same package) run concurrently.

---

## Worked Examples

### Example 1: `pnpm run --filter "a..." build`

**Setup:** a (has `build`) → depends on b (no `build`) → depends on c (has `build`)

**Stage 1 — Package Selection:**

1. Parse `"a..."` → `{ namePattern: "a", includeDependencies: true }`
2. Build full dependency graph: a→b, b→c
3. Entry packages: match name "a" → `[a]`
4. Walk dependencies from a: a→b→c. `walkedDependencies = {a, b, c}`
5. `selectedProjectsGraph = { a, b, c }`

**Stage 2 — Task Matching:**

1. Topological sort of {a, b, c} within selected graph: chunks = `[[c], [b], [a]]` (dependencies first)
2. Chunk [c]: `getSpecifiedScripts(c.scripts, "build")` → `["build"]` → run c's build. `hasCommand = 1`
3. Chunk [b]: `getSpecifiedScripts(b.scripts, "build")` → `[]` → skip. b is marked `'skipped'`.
4. Chunk [a]: `getSpecifiedScripts(a.scripts, "build")` → `["build"]` → run a's build. `hasCommand = 2`
5. `hasCommand > 0`, no error.

**Result:** c's `build` runs first, b is skipped, then a's `build` runs.

### Example 2: `pnpm run --filter "a..." build`

**Setup:** a (no `build`) → depends on b (has `build`)

**Stage 1 — Package Selection:**

1. Parse `"a..."` → `{ namePattern: "a", includeDependencies: true }`
2. Entry packages: `[a]`
3. Walk dependencies: a→b. `walkedDependencies = {a, b}`
4. `selectedProjectsGraph = { a, b }`

**Stage 2 — Task Matching:**

1. Topological sort: chunks = `[[b], [a]]`
2. Chunk [b]: `getSpecifiedScripts(b.scripts, "build")` → `["build"]` → run. `hasCommand = 1`
3. Chunk [a]: `getSpecifiedScripts(a.scripts, "build")` → `[]` → skip.
4. `hasCommand > 0`, no error.

**Result:** b's `build` runs, a is skipped.

### Example 3: `pnpm run --filter "a..." /glob/`

**Setup:** a → depends on b. Package a has script `taskA` matching `/glob/`. Package b has script `taskB` matching `/glob/`. Neither has the other's task.

**Stage 1 — Package Selection:**

1. Parse `"a..."` → `{ namePattern: "a", includeDependencies: true }`
2. Entry packages: `[a]`
3. Walk dependencies: a→b. `walkedDependencies = {a, b}`
4. `selectedProjectsGraph = { a, b }`

**Stage 2 — Task Matching:**

1. `scriptName = "/glob/"`. This is a regex literal.
2. `tryBuildRegExpFromCommand("/glob/")` → `RegExp("glob")`
3. Topological sort: chunks = `[[b], [a]]`
4. Chunk [b]: `getSpecifiedScripts(b.scripts, "/glob/")`:
   - No exact match for literal `"/glob/"` in scripts
   - Regex match: filter b's script keys by `RegExp("glob")` → finds `taskB` → `["taskB"]`
   - Run b's `taskB`. `hasCommand = 1`
5. Chunk [a]: `getSpecifiedScripts(a.scripts, "/glob/")`:
   - No exact match
   - Regex match: filter a's script keys by `RegExp("glob")` → finds `taskA` → `["taskA"]`
   - Run a's `taskA`. `hasCommand = 2`
6. `hasCommand > 0`, no error.

**Result:** b's `taskB` runs first, then a's `taskA`. Each package independently matches its own scripts against the regex. The regex is applied per-package, so different packages can match different script names.

**Ordering note:** Even though `taskA` and `taskB` have different names, b's `taskB` still runs before a's `taskA` because the ordering is at the package level. Package b is in an earlier topological chunk than a (since a depends on b). All scripts matched in a package inherit that package's position in the execution order.

### Example 4: `pnpm run --filter a --filter b build`

**Setup:** a depends on b. Both have `build`.

**Stage 1 — Package Selection:**

1. Two `--filter` flags → two `WorkspaceFilter` objects, both `followProdDepsOnly: false`.
2. Parse selectors:
   - `"a"` → `{ namePattern: "a" }` (no `...`)
   - `"b"` → `{ namePattern: "b" }` (no `...`)
3. Both are include selectors. `_filterGraph` processes them sequentially:
   - **Selector 1 (a):** Entry = [a]. Neither `includeDependencies` nor `includeDependents` → `cherryPickedPackages = [a]`.
   - **Selector 2 (b):** Entry = [b]. Same → `cherryPickedPackages = [a, b]`.
4. No graph walking occurs — both packages are cherry-picked.
5. `selectedProjectsGraph = { a, b }` — both retain their original dependency edges from the full graph (a→b edge is preserved).

**Stage 2 — Task Matching:**

1. Topological sort over {a, b}. Edge a→b is within the selected set. Chunks: `[[b], [a]]`.
2. Chunk [b]: run `build`. Chunk [a]: run `build`.

**Key insight:** Even though neither filter used `...` (no dependency expansion), the topological sort still respects the a→b dependency edge. Cherry-picking packages does not remove their dependency relationships — the `selectedProjectsGraph` retains the original edges from the full graph, and [`sequenceGraph`](workspace/sort-packages/src/index.ts#L5-L16) filters edges to only those between selected packages. So b's `build` runs before a's `build`.

### Example 5: `pnpm run --filter 'app...' --filter 'cli' build`

**Setup:** Workspace has packages: app, lib, core, utils, cli. app→lib→core→utils (chain). cli→core (separate). All have `build`.

**Stage 1 — Package Selection:**

1. Two `--filter` flags produce two `WorkspaceFilter` objects, both with `followProdDepsOnly: false`. They share the **same** graph (the full "all" graph).
2. Parse selectors:
   - `"app..."` → `{ namePattern: "app", includeDependencies: true }`
   - `"cli"` → `{ namePattern: "cli" }` (no `...`, no `!`, no `^`)
3. Both are include selectors (no `exclude`). `_filterGraph` processes them sequentially:
   - **Selector 1 (app...):** Entry = [app]. Walk dependencies: app→lib→core→utils. `walkedDependencies = {app, lib, core, utils}`.
   - **Selector 2 (cli):** Entry = [cli]. Neither `includeDependencies` nor `includeDependents` → push to `cherryPickedPackages = [cli]`.
4. Combine: union of all collections → `{app, lib, core, utils, cli}`.
5. `selectedProjectsGraph = { app, lib, core, utils, cli }` — **all five** packages, each retaining their original dependency edges from the full graph.

**Stage 2 — Task Matching:**

1. Topological sort over the selected graph. Edges within the selected set: app→lib, lib→core, core→utils, cli→core.
   - Chunks: `[[utils], [core], [lib, cli], [app]]`
   - Note: `lib` and `cli` are in the **same chunk** — they both depend on `core` but not on each other, so they can run in parallel.
2. Chunk [utils]: run `build`. Chunk [core]: run `build`. Chunk [lib, cli]: run both `build` scripts concurrently. Chunk [app]: run `build`.

**Key insight:** Even though `cli` was selected without `...` (cherry-picked, not graph-expanded), the topological sort still respects its dependency on `core` because the sort operates on the `selectedProjectsGraph` which retains the original dependency edges. Multiple `--filter` flags with the same `followProdDepsOnly` value contribute to the same `_filterGraph` call and their results are unioned together.

### Examples 6-8: `--filter` / `--filter-prod` mix with devDependencies

**Common setup for all three:** b is a **devDependency** of a. Both have `build`.

Each selected package's node comes from the graph of whichever filter type selected it. The a→b edge only exists in the full graph (from `--filter`), not the prod graph (from `--filter-prod`). So the edge in the final `selectedProjectsGraph` depends on which graph **a's node** came from — since a is the package that _declares_ the dependency.

### Example 6: `pnpm run --filter-prod a --filter-prod b build`

**Stage 1:** Both selectors have `followProdDepsOnly: true`. A single prod graph is built (`ignoreDevDeps: true`). a's node has no edge to b. Both cherry-picked into `selectedProjectsGraph`.

**Stage 2:** `sortPackages` sees no edge → **1 chunk** containing both. They run concurrently.

### Example 7: `pnpm run --filter a --filter-prod b build`

**Stage 1:** The selectors are split:

- `"a"` → `allPackageSelectors` (full graph). a's node comes from the full graph → its `dependencies` array **includes** b (devDep edge present).
- `"b"` → `prodPackageSelectors` (prod graph). b's node comes from the prod graph.

Merge: `{ ...prodGraph(b), ...fullGraph(a) }`. No overlap, so a's node (with devDep edge) and b's node are both present.

**Stage 2:** `sortPackages` sees edge a→b → **2 chunks**: `[[b], [a]]`. b's `build` runs first.

### Example 8: `pnpm run --filter-prod a --filter b build`

**Stage 1:** The selectors are split:

- `"a"` → `prodPackageSelectors` (prod graph). a's node comes from the prod graph → its `dependencies` array **excludes** b (devDep edge absent).
- `"b"` → `allPackageSelectors` (full graph). b's node comes from the full graph.

Merge: `{ ...prodGraph(a), ...fullGraph(b) }`. a's node has no edge to b.

**Stage 2:** `sortPackages` sees no edge → **1 chunk** containing both. They run concurrently.

**Key insight across 6-8:** The dependency edge a→b exists in the final graph **only when a's node comes from the full graph** (i.e. a was selected via `--filter`, not `--filter-prod`). It does not matter which flag selected b. This asymmetry comes from the merge in `filterPkgsBySelectorObjects`: each node retains the edges from whichever graph (full vs prod) it was selected from.

---

## Key Design Insight

The two stages are fully decoupled:

- **Stage 1 (package selection)** answers: "Which workspace packages should be considered?" It uses the dependency graph and filter patterns, and knows nothing about scripts.
- **Stage 2 (task matching)** answers: "Within each selected package, which scripts should run?" It uses the script name (exact or regex) against each package's `scripts` field, and skips packages without matches.

This means `--filter "a..."` always selects a and all its (transitive) dependencies regardless of whether they have the requested script. Packages without the script are silently skipped (unless `requiredScripts` is configured or no package at all has the script).

---

## Key Source Files

| Component                          | Path                                                                                                                               |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| CLI arg parsing                    | [cli/parse-cli-args/src/index.ts](cli/parse-cli-args/src/index.ts)                                                                 |
| Filter wiring in main              | [pnpm/src/main.ts:194-251](pnpm/src/main.ts#L194-L251)                                                                             |
| Selector parsing                   | [workspace/filter-workspace-packages/src/parsePackageSelector.ts](workspace/filter-workspace-packages/src/parsePackageSelector.ts) |
| Package filtering core             | [workspace/filter-workspace-packages/src/index.ts](workspace/filter-workspace-packages/src/index.ts)                               |
| Dependency graph construction      | [workspace/pkgs-graph/src/index.ts](workspace/pkgs-graph/src/index.ts)                                                             |
| Name pattern matcher               | [config/matcher/src/index.ts](config/matcher/src/index.ts)                                                                         |
| Topological sort                   | [workspace/sort-packages/src/index.ts](workspace/sort-packages/src/index.ts)                                                       |
| Run command handler                | [exec/plugin-commands-script-runners/src/run.ts](exec/plugin-commands-script-runners/src/run.ts)                                   |
| Recursive runner + script matching | [exec/plugin-commands-script-runners/src/runRecursive.ts](exec/plugin-commands-script-runners/src/runRecursive.ts)                 |
| Regex command parser               | [exec/plugin-commands-script-runners/src/regexpCommand.ts](exec/plugin-commands-script-runners/src/regexpCommand.ts)               |

---

## Relevant Tests

### Selector Parsing

- [workspace/filter-workspace-packages/test/parsePackageSelector.ts](workspace/filter-workspace-packages/test/parsePackageSelector.ts) — 16 fixture-driven tests covering all selector syntax: name, `...`, `^`, `!`, `{dir}`, `[diff]`, and combinations.

### Package Filtering (graph traversal)

- [workspace/filter-workspace-packages/test/index.ts](workspace/filter-workspace-packages/test/index.ts) — Tests for `filterWorkspacePackages`: dependencies, dependents, combined deps+dependents, self-exclusion, by-name, by-directory (exact and glob), git-diff filtering, exclusion patterns, unmatched filter reporting.

### Dependency Graph Construction

- [workspace/pkgs-graph/test/index.ts](workspace/pkgs-graph/test/index.ts) — Tests for `createPkgGraph`: basic deps, peer deps, local directory deps, `workspace:` protocol, `ignoreDevDeps: true`, `linkWorkspacePackages: false`, prerelease version matching.

### Name Pattern Matcher

- [config/matcher/test/index.ts](config/matcher/test/index.ts) — Tests for `createMatcher`: wildcard (`*`), glob patterns, exact match, negation (`!`), multiple patterns.

### Topological Sort

- [deps/graph-sequencer/test/index.ts](deps/graph-sequencer/test/index.ts) — 18+ tests for `graphSequencer`: cycles, subgraph sequencing, independent nodes, multi-dependency chains.

### Run Command (unit)

- [exec/plugin-commands-script-runners/test/runRecursive.ts](exec/plugin-commands-script-runners/test/runRecursive.ts) — 29 tests: basic recursive run, reversed, concurrent, filtering, `--if-present`, `--bail`, `requiredScripts`, `--resume-from`, RegExp selectors, report summary.
- [exec/plugin-commands-script-runners/test/index.ts](exec/plugin-commands-script-runners/test/index.ts) — 21 tests: single-package run, exit codes, RegExp script selectors (including invalid flags), `--if-present`, command suggestions.

### Regex Script Matching

- Covered in both test files above. Key tests: `pnpm run with RegExp script selector should work` and 8 tests for invalid regex flags.

### `--filter-prod`

- [pnpm/test/filterProd.test.ts](pnpm/test/filterProd.test.ts) — E2E tests comparing `--filter` vs `--filter-prod` with a 4-project graph, verifying devDependencies inclusion/exclusion.

### E2E / Integration

- [pnpm/test/recursive/run.ts](pnpm/test/recursive/run.ts) — CLI-level integration tests for `pnpm run` in recursive mode.
- [pnpm/test/recursive/filter.ts](pnpm/test/recursive/filter.ts) — CLI-level integration tests for recursive filtering.

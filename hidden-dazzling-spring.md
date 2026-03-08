# Plan: Resolve Globs to Workspace-Root-Relative at Task Graph Stage

## Context

`ResolvedInputConfig` currently stores raw user-provided glob strings (e.g. `src/**/*.ts`, `../shared/dist/**`) relative to the package directory. These are resolved at execution time using the now-removed `AnchoredGlob` type. The code is broken — `AnchoredGlob` was removed from `vite_glob` (commit f880ca10) but 3 files in `vite_task` still reference it.

Moving glob resolution to the task_graph stage makes globs workspace-root-relative, eliminating `AnchoredGlob`, `glob_base`, and `base_dir` from the execution pipeline.

## Algorithm: Resolve a single glob to workspace-root-relative

```
partition(glob) → (invariant_prefix, variant)
joined = package_dir.join(invariant_prefix)
cleaned = path_clean::clean(joined)
stripped = cleaned.strip_prefix(workspace_root)  // error if fails
result = wax::escape(stripped) + "/" + variant    // or just escaped if no variant
```

`AbsolutePath::strip_prefix` already normalizes separators, so no special Windows handling needed.

---

## Steps

### 1. Add deps to `vite_task_graph`

**File:** `crates/vite_task_graph/Cargo.toml`

Add `wax` and `path-clean` (already workspace deps).

### 2. Add glob resolution to `ResolvedInputConfig`

**File:** `crates/vite_task_graph/src/config/mod.rs`

- Add error variants to `ResolveTaskConfigError`:
  - `GlobOutsideWorkspace { pattern: Str }` — "glob pattern '...' resolves outside the workspace root"
  - `InvalidGlob { pattern: Str, source: wax::BuildError }`

- Add helper `resolve_glob_to_workspace_relative(pattern, package_dir, workspace_root) -> Result<Str, ResolveTaskConfigError>` implementing the algorithm above.

- Change `from_user_config` signature to accept `package_dir` and `workspace_root`, return `Result`. Each raw glob goes through `resolve_glob_to_workspace_relative`.

- Change `ResolvedTaskOptions::resolve()` to accept `workspace_root`, return `Result`.

- Change `ResolvedTaskConfig::resolve()` and `resolve_package_json_script()` to accept `workspace_root`, return `Result`.

### 3. Thread `workspace_root` in `IndexedTaskGraph::load()`

**File:** `crates/vite_task_graph/src/lib.rs`

Pass `&workspace_root.path` to `ResolvedTaskConfig::resolve()` (line ~275) and `resolve_package_json_script()` (line ~307). Propagate the new `Result`.

### 4. Remove `glob_base` from `CacheMetadata`

**File:** `crates/vite_task_plan/src/cache_metadata.rs`

Remove `glob_base: Arc<AbsolutePath>` field.

### 5. Remove `glob_base` from plan construction

**File:** `crates/vite_task_plan/src/plan.rs`

Remove `glob_base: Arc::clone(package_path)` at line ~558.

### 6. Remove `glob_base` from `CacheEntryKey` + bump DB version

**File:** `crates/vite_task/src/session/cache/mod.rs`

- Remove `glob_base: RelativePathBuf` from `CacheEntryKey` (line 43).
- Simplify `from_metadata()` — remove glob_base strip_prefix logic (lines 56-66).
- Bump cache version: `1..=8` → `1..=9`, `9 => break` → `10 => break`, new DB `PRAGMA user_version = 10`, unrecognized `10..` → `11..`.

### 7. Simplify `compute_globbed_inputs()`

**File:** `crates/vite_task/src/session/execute/glob_inputs.rs`

- Remove `use vite_glob::AnchoredGlob` import.
- Remove `base_dir` parameter — globs are already workspace-root-relative.
- For each positive glob: `Glob::new(pattern).walk(workspace_root).not(negatives)` — `.not()` supports directory pruning for efficiency. No partition/join/clean.
- Parse all negative globs upfront as `Vec<Glob<'static>>` and pass to `.not()` for each positive walk.

### 8. Simplify fspy filtering in `spawn.rs`

**File:** `crates/vite_task/src/session/execute/spawn.rs`

- Remove `use vite_glob::AnchoredGlob`.
- Change `resolved_negatives: &[AnchoredGlob]` → `resolved_negatives: &[wax::Glob<'static>]`.
- At lines 216-224: match `relative_path` directly against negative globs (both are workspace-relative). Remove `path_clean`, `workspace_root.join`, `AbsolutePath::new`.

### 9. Simplify `execute_spawn()` in `mod.rs`

**File:** `crates/vite_task/src/session/execute/mod.rs`

- Remove `resolve_negative_globs()` function (lines 425-434).
- Update `compute_globbed_inputs` call: remove `cache_metadata.glob_base`, pass `cache_base_path` as workspace root.
- Build negative globs inline: `negative_globs.iter().map(|p| Glob::new(p).into_owned()).collect()`.

### 10. Update tests

- **`config/mod.rs` tests**: Add `package_dir` + `workspace_root` params. Assert workspace-root-relative patterns. Add test for `..` resolution and outside-workspace error.
- **`glob_inputs.rs` tests**: Remove `base_dir` param, pass workspace-root-relative globs.
- **Plan snapshots**: `INSTA_UPDATE=always cargo test -p vite_task_plan --test plan_snapshots` (removes `glob_base` lines).
- **E2E snapshots**: `INSTA_UPDATE=always cargo test -p vite_task_bin --test e2e_snapshots`.

---

## Verification

```bash
cargo check --all-targets
cargo test
INSTA_UPDATE=always cargo test -p vite_task_plan --test plan_snapshots
INSTA_UPDATE=always cargo test -p vite_task_bin --test e2e_snapshots
just lint
```

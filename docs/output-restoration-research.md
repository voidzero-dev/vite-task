# Output Restoration: Compatibility with Real Build Tools

## Background

Output restoration automatically archives files produced by a cached task and restores them on cache hit.
When a task runs and creates `dist/`, those files are saved as a `tar.zst` archive in the cache directory.
On subsequent cache hits the archive is extracted, skipping the need to re-execute the task.

The feature works end-to-end when tested with simple write-only commands (`vtt write-file dist/out.txt built`),
but **fails with real build tools** (Vite 8, tsdown) in the default auto-detection mode.

## The Problem

Deleting the output directory (`dist/`) between runs causes a **cache miss** instead of triggering output restoration.

```
~/packages/vite-app$ vite build
...
✓ built in 34ms

# delete dist, run again:

~/packages/vite-app$ vite build ○ cache miss: 'assets' removed from 'packages/vite-app/dist', executing
```

The archived output files exist in the cache and could be restored, but the cache validation rejects the entry before restoration has a chance to run.

## Root Cause

Build tools **read from their output directory** during execution. fspy captures these reads and records them as inferred inputs. When the output directory is later deleted, the inferred input fingerprint no longer matches, producing a cache miss.

### How the cache validation pipeline works

1. **Cache entry lookup** — match by `CacheEntryKey` (spawn fingerprint + input config + output config)
2. **Globbed input validation** — compare stored file hashes against current state for explicit input globs
3. **Post-run fingerprint validation** — compare stored fspy-inferred input fingerprints against current filesystem state
4. **If all pass** → cache hit → replay terminal output → **extract output archive**

The failure occurs at step 3. The stored fingerprint records `packages/app/dist` as `Folder(Some({assets: Dir, index.html: File}))`. After deletion the current fingerprint is `NotFound`. The mismatch is reported before output restoration at step 4 can execute.

### Why build tools read from the output directory

Both Vite 8 and tsdown follow the same pattern: **clean the output directory before writing new files**, then **report compressed sizes after writing**.

#### Vite 8 (`vite build`)

1. **Directory cleanup** (`emptyDir()`, called from `prepareOutDirPlugin` during `renderStart`)
   - `fs.readdirSync(outDir)` enumerates entries so they can be deleted before the new build
   - Controlled by `emptyOutDir` (default: `true`)
   - This is the primary read that causes `packages/app/dist` to appear in fspy's `path_reads`

2. **Compressed size reporting** (rolldown's builtin `viteReporterPlugin`, Rust-native)
   - After writing output files, the reporter reads each file back to compute gzip/brotli sizes
   - Produces the `dist/index.html  0.15 kB │ gzip: 0.14 kB` lines
   - Controlled by `reportCompressedSize` (default: `true`)
   - This causes reads on individual output files like `dist/index.html`, `dist/assets/index-xxx.js`

3. **Public directory copy** (`copyDir()`)
   - If `copyPublicDir` is enabled (default: `true`), reads the public dir to copy into outDir

#### tsdown

1. **Directory cleanup** (`cleanOutDir()` via `tinyglobby`'s `glob()`)
   - Enumerates all files in the output directory with `onlyFiles: false` before deleting them
   - Default `clean: true` triggers this
   - This is the primary read that causes `packages/lib/dist` to appear in fspy's `path_reads`

2. **Shebang permission check** (`ShebangPlugin`'s `writeBundle` hook)
   - Calls `access()` on output files to check existence before setting execute permissions
   - Only applies to entry chunks with shebang directives

### Why the read-write overlap check doesn't catch this

The overlap check at `execute_spawn` (mod.rs:486-488) looks for exact path matches between `path_reads` and `path_writes`:

```rust
pa.path_reads.keys().find(|p| pa.path_writes.contains(*p))
```

fspy reports these as separate paths:

- **Read**: `packages/app/dist` (the directory itself, via `readdirSync`)
- **Write**: `packages/app/dist/index.html`, `packages/app/dist/assets/index-xxx.js` (individual files)

Since `packages/app/dist` ≠ `packages/app/dist/index.html`, no overlap is detected, and caching proceeds.

### Why `should_ignore_entry` doesn't help

`fingerprint.rs:185-187` filters `dist` when listed as a directory entry of a **parent**:

```rust
fn should_ignore_entry(name: &[u8]) -> bool {
    matches!(name, b"." | b".." | b".DS_Store") || name.eq_ignore_ascii_case(b"dist")
}
```

This prevents the fingerprint of `packages/app/` from changing when `dist` appears or disappears inside it. But it does not help when `packages/app/dist` itself is a direct key in `inferred_inputs` — that path is fingerprinted independently, and its transition from `Folder(...)` to `NotFound` is a mismatch.

### Why the existing e2e test passes

The `output-cache-test` fixture uses `vtt write-file dist/out.txt built`, a simple write-only operation. `vtt write-file` never reads from `dist/`, so fspy only records it as a write. The directory never appears in `path_reads`, the post-run fingerprint doesn't include it, and cache validation succeeds after deletion.

This does not reflect real build tool behavior.

## Behavior Matrix

All tests performed with Vite 8.0.8 and tsdown 0.12.9. "Cache hit after deleting dist" means output restoration can work.

| `input`        | `output`        | fspy enabled | Cache hit after deleting dist |
| -------------- | --------------- | ------------ | ----------------------------- |
| auto (default) | auto (default)  | yes          | **no**                        |
| auto (default) | `["dist/**"]`   | yes          | **no**                        |
| `["src/**"]`   | auto (default)  | yes          | **no**                        |
| `["src/**"]`   | `["dist/**"]`   | no           | **yes**                       |
| `["src/**"]`   | `[]` (disabled) | no           | yes (but no restoration)      |

The only configuration that works requires **both** explicit input globs **and** explicit output globs, which disables fspy entirely. Any configuration that enables fspy (for either auto-input or auto-output detection) causes the output directory reads to pollute the inferred input set.

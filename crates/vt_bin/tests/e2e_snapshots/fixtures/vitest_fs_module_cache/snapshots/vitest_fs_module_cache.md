# vitest_fs_module_cache

Vitest 5 with `fsModuleCache` writes each transformed module to `node_modules/.vitest-cache/.tmp-*`, renames it to `node_modules/.vitest-cache/<hash>` and reads it back. It also writes `_metadata.json` there and rewrites `node_modules/.vite/vitest/<hash>/results.json`.

## `vt run test`

```
$ vitest run

 RUN  v<version> <workspace>

 ✓ src/add.test.js (1 test) <duration>
```

## `vtt rm -r node_modules/.vitest-cache node_modules/.vite/vitest`

remove Vitest's caches, as on a clean checkout (vp's own cache is in `node_modules/.vite/task-cache`)

```
```

## `vt run test`

several inputs are missing and vt names whichever it finds first, so the miss reason is redacted

```
$ vitest run ◉ cache hit, replaying

 RUN  v<version> <workspace>

 ✓ src/add.test.js (1 test) <duration>

---
vt run: cache hit.
```

## `vtt rm node_modules/.vitest-cache/_metadata.json`

remove the metadata, whose place among the hashed names in the listing varies

```
```

## `vtt list-dir node_modules/.vitest-cache`

```
<sha1>
```

# legacy_top_level_cache_is_migrated

A cache left by an older Vite+ build in the pre-versioned layout (a top-level `cache.db` and an orphaned output archive) is swept on the first run, and the cache is recreated under a per-schema-version subdirectory. This is what lets builds that pin different Vite+ versions coexist instead of aborting on each other's database. The assertions avoid naming the version directory so they survive a schema-version bump.

## `vtt write-file node_modules/.vite/task-cache/cache.db legacy-db`

plant a legacy top-level cache database

```
```

## `vtt write-file node_modules/.vite/task-cache/stale.tar.zst legacy-archive`

plant an orphaned output archive next to it

```
```

## `vtt list-dir node_modules/.vite/task-cache`

before: legacy files sit directly in the cache directory

```
cache.db
stale.tar.zst
```

## `vt run cached-task`

first run migrates the layout, then executes (cache miss)

```
$ vtt print-file test.txt
test content
```

## `vtt list-dir node_modules/.vite/task-cache --ext .db`

after: the legacy top-level database is gone

```
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst`

after: the orphaned top-level archive is gone

```
```

## `vtt list-dir node_modules/.vite/task-cache --ext .db --recursive`

the cache database was recreated inside the per-version subdirectory

```
cache.db
```

## `vt run cached-task`

cache hit from the migrated, versioned cache

```
$ vtt print-file test.txt ◉ cache hit, replaying
test content

---
vt run: cache hit.
```

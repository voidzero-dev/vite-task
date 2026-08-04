# leftover_cache_from_another_version_is_ignored

A cache left at the old top-level location by a different Vite+ version is ignored: this build reads and writes only its own per-schema-version subdirectory, so the leftover database never aborts the run (the bug behind vite-plus#1785) and caching still works.

## `vtt write-file node_modules/.vite/task-cache/cache.db 'cache from another version'`

simulate a leftover cache database from a different Vite+ version

```
```

## `vt run cached-task`

first run is unaffected by the leftover (cache miss)

```
$ vtt print-file test.txt
test content
```

## `vt run cached-task`

second run hits this build's own per-version cache

```
$ vtt print-file test.txt ◉ cache hit, replaying
test content

---
vt run: cache hit.
```

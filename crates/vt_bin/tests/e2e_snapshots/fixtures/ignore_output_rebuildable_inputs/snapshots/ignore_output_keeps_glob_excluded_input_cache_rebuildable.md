# ignore_output_keeps_glob_excluded_input_cache_rebuildable

The task reports only `ignoreOutput` for `tool/cache`. An input negative glob excludes the cache file it reads and writes, so `tool` can still be rebuilt.

## `vt run exclude-cache-input`

```
$ node ignored-cache.mjs input-negative
```

## `vtt rm -r tool`

```
```

## `vt run exclude-cache-input`

```
$ node ignored-cache.mjs input-negative ◉ cache hit, replaying

---
vt run: cache hit.
```

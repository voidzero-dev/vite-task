# ignore_output_keeps_ignored_input_cache_rebuildable

The task reads and writes files under `tool/cache` and reports the cache folder with both `ignoreInput` and `ignoreOutput`. It lists `tool` and the workspace root. Removing `tool` simulates a clean checkout and removes an entry from the root listing.

## `vt run ignore-cache`

```
$ node ignored-cache.mjs
```

## `vtt rm -r tool`

```
```

## `vt run ignore-cache`

```
$ node ignored-cache.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file tool/cache`

```
tool/cache: missing
```

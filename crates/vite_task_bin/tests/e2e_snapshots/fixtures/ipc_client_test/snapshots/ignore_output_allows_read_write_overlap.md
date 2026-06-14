# ignore_output_allows_read_write_overlap

Exercises `ignoreOutput`. The task reads and writes `sidecar/tmp.txt`; without the ignore the runner's read-write overlap check would refuse to cache the run. The task also writes `dist/out.txt`, but output caching is disabled here so a cache hit must not restore it.

## `vt run ignore-output`

first run populates the cache

```
$ node scripts/ignore_output.mjs
```

## `vtt rm dist/out.txt`

remove the untracked output so a cache-hit restore would be visible

```
```

## `vt run ignore-output`

cache hit: sidecar/ writes were ignored

```
$ node scripts/ignore_output.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

not restored because this task has output: []

```
dist/out.txt: not found
```

# ignore_output_allows_read_write_overlap

Exercises `ignoreOutput` with auto output tracking. The task reads and writes `sidecar/tmp.txt`; without the ignore the runner's read-write overlap check would refuse to cache the run. The task also writes `dist/out.txt`, which should be auto-archived and restored on a cache hit.

## `vt run ignore-output`

first run populates the cache

```
$ node scripts/ignore_output.mjs
```

## `vtt rm dist/out.txt`

remove the auto output so restoration is observable

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

restored from the auto output archive

```
ok
```

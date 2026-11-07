# nothing_runs_or_is_written

## `vt run --dry-run build`

No cache yet. Nothing runs, so the file isn't printed.

```
$ vtt print-file src/a.txt
```

## `vt run build`

A real run caches the task.

```
$ vtt print-file src/a.txt
original
```

## `vtt list-dir node_modules/.vite/task-cache --recursive`

```
cache.db
db_open.lock
last-summary.json
```

## `vt run --dry-run build`

A hit isn't replayed.

```
$ vtt print-file src/a.txt ◉ cache hit

---
vt run: cache hit, nothing was run.
```

## `vtt write-file src/a.txt changed`

```
```

## `vt run --dry-run build`

A miss names the changed file, without running.

```
$ vtt print-file src/a.txt ○ cache miss: 'src/a.txt' modified
```

## `vt run --dry-run build`

Nothing was recorded, so it's the same miss again.

```
$ vtt print-file src/a.txt ○ cache miss: 'src/a.txt' modified
```

## `vtt list-dir node_modules/.vite/task-cache --recursive`

The cache folder holds the same files.

```
cache.db
db_open.lock
last-summary.json
```

## `vt run --last-details`

The last run is still the real one.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] dry-run#build: $ vtt print-file src/a.txt ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

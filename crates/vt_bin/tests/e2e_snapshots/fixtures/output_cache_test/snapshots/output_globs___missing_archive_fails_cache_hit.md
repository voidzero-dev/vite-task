# output_globs___missing_archive_fails_cache_hit

When the archive of a cache hit is missing, its output files can't be restored. The run fails and suggests clearing the cache, after which the task executes again.

## `vt run build`

first run — cache miss, writes the archive

```
$ vtt write-file dist/output.txt built
```

## `vtt rm --ext .tar.zst node_modules/.vite/task-cache`

delete the archive

```
```

## `vt run build`

second run — cache hit, but restoring fails, so the run fails

**Exit code:** 1

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying
✗ Cache restore failed. Run `vt cache clean` to clear the cache: failed to extract the output archive: <os error>
```

## `vt run --last-details`

the task is reported as failed, with its error

**Exit code:** 1

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 0 cache misses • 1 failed
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] output-cache-test#build: $ vtt write-file dist/output.txt built
      → Cache hit, but the outputs couldn't be restored
      ✗ Error: Cache restore failed. Run `vt cache clean` to clear the cache
        ↳ failed to extract the output archive
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt cache clean`

clear the cache, as the error suggests

```
```

## `vt run build`

third run — cache miss, so the task executes

```
$ vtt write-file dist/output.txt built
```

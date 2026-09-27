# output_globs___missing_archive_fails_cache_hit

When the archive of a cache hit is missing, its output files can't be restored. The run fails, and the entry is removed from the cache, so the next run executes the task instead of hitting the entry again.

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
✗ Cache restore failed: failed to extract the output archive: <os error>
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
      ✗ Error: Cache restore failed
        ↳ failed to extract the output archive
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run build`

third run — the entry was removed, so the task executes

```
$ vtt write-file dist/output.txt built
```

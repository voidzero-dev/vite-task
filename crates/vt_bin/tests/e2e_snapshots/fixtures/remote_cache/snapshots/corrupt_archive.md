# corrupt_archive

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 200 not_found
[remote-cache] POST /store 200
```

## `vtt write-file remote-cache/blobs/1 corrupt`

Overwrite the stored archive.

```
```

## `vt cache clean`

```
```

## `remote-cache-server vt run build`

The downloaded archive doesn't decode, so the task reruns.

```
$ vtt write-file dist/output.txt built ○ cache miss: downloaded archive is corrupt, executing

[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

## `vt run --last-details`

The details include the underlying error.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Cache miss: downloaded archive is corrupt
        ↳ Unknown frame descriptor
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst --recursive`

Only the rerun's archive is on disk. The corrupt download was removed.

```
<uuid>.tar.zst
```

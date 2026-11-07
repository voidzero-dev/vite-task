# corrupt_archive

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `remote-cache-server corrupt-blob 1`

Overwrite the stored archive.

```
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run build`

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

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Cache miss: downloaded archive is corrupt
        ↳ Unknown frame descriptor
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tmp --recursive`

The corrupt download was removed.

```
```

## `remote-cache-server stop`

```
```

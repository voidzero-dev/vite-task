# expired_after_fetch

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `remote-cache-server expire-entry --before-download 1`

The entry expires after the next fetch returns it, before its archive is downloaded.

```
```

## `remote-cache-server run vt run build`

The fetch finds the entry, but its archive is gone, so the task reruns.

```
$ vtt write-file dist/output.txt built ○ cache miss: remote cache download failed, executing

[remote-cache] POST /fetch 200 exact [1]
[remote-cache] GET /blob/1 404
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
      → Cache miss: remote cache download failed
        ↳ HTTP status 404
        ↳ Not found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `remote-cache-server stop`

```
```

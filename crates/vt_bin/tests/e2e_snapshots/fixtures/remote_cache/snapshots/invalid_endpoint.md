# invalid_endpoint

## `VP_REMOTE_CACHE=read-write VP_REMOTE_CACHE_URL=cache.example/projects/test vt run build`

The failed upload is a warning. The task succeeds.

```
$ vtt write-file dist/output.txt built

---
vt run: remote-cache#build not uploaded to the remote cache: invalid endpoint. (Run `vt run --last-details` for full details)
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
      → Cache miss: no previous cache entry found
      ⚠ Not uploaded to the remote cache: invalid endpoint: relative URL without a base
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `VP_REMOTE_CACHE=read-write VP_REMOTE_CACHE_URL=cache.example/projects/test vt run build`

The local cache was updated. Hits never upload.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

# ctrl_c_during_upload

## `VP_REMOTE_CACHE=read-write remote-cache-server vtt stalled-remote-cache --stall /store vt run build`

The proxy forwards the fetch to the backend, which has no entry, but never forwards the upload. Ctrl-C cancels it while vt run waits.

**→ expect-milestone:** `uploads-pending`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish...
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish...
---
vt run: remote-cache#build not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
```

## `vt run --last-details`

The details show why build wasn't uploaded.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Cache miss: no previous cache entry found
      ⚠ Not uploaded to the remote cache: interrupted
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `vt run build`

The entry is still in the local cache.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

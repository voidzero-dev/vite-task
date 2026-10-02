# ctrl_c_during_upload

## `VP_REMOTE_CACHE=read-write vtt stalled-remote-cache --fetch-miss vt run build`

The fetch is a miss, but the endpoint never responds to the upload. Ctrl-C cancels it while vt run waits.

**→ expect-milestone:** `uploads-pending`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
---
vt run: remote-cache#build not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
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

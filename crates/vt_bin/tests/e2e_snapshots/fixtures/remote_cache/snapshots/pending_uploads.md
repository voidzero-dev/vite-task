# pending_uploads

## `VP_REMOTE_CACHE=read-write REMOTE_CACHE_STORE_DELAY_MS=2000 remote-cache-server vt run all`

check doesn't wait for build's upload. Both uploads are still running when check finishes, and vt run waits for them.

```
$ vtt write-file dist/output.txt built

$ vtt print checked
checked

Waiting for 2 remote cache uploads to finish (Ctrl-C to cancel)...
---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
[remote-cache] POST /store 200
```

## `vt run --last-details`

Both uploads succeeded.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 0 cache hits • 2 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Cache miss: no previous cache entry found
  ·······················································
  [2] remote-cache#check: $ vtt print checked ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

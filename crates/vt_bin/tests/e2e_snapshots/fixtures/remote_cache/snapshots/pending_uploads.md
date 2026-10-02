# pending_uploads

## `VP_REMOTE_CACHE=read-write vtt stalled-remote-cache --fetch-miss vt run all`

The endpoint never responds to the uploads. check doesn't wait for build's upload, so both are still running when check finishes, and vt run waits for them until Ctrl-C.

**→ expect-milestone:** `uploads-pending`

```
$ vtt write-file dist/output.txt built

$ vtt print checked
checked

Waiting for 2 remote cache uploads to finish (Ctrl-C to cancel)...
```

**← write-key:** `ctrl-c`

```
$ vtt write-file dist/output.txt built

$ vtt print checked
checked

Waiting for 2 remote cache uploads to finish (Ctrl-C to cancel)...
---
vt run: 0/2 cache hit (0%). remote-cache#build (and 1 more) not uploaded to the remote cache: interrupted. (Run `vt run --last-details` for full details)
```

## `vt run --last-details`

Both uploads were cancelled.

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
      ⚠ Not uploaded to the remote cache: interrupted
  ·······················································
  [2] remote-cache#check: $ vtt print checked ✓
      → Cache miss: no previous cache entry found
      ⚠ Not uploaded to the remote cache: interrupted
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

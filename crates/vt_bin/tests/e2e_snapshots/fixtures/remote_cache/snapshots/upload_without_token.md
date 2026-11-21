# upload_without_token

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run vt run build`

Outside GitHub Actions, the upload has no token, so the backend rejects it. The task succeeds.

```
$ vtt write-file dist/output.txt built

---
vt run: remote-cache#build not uploaded to the remote cache: HTTP status 401. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[remote-cache] POST /store 401
```

## `vt run --last-details`

The details include the backend's message.

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
      ⚠ Not uploaded to the remote cache: HTTP status 401
        ↳ Invalid credentials
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `remote-cache-server stop`

```
```

# pull_request_upload

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write ACTIONS_ID_TOKEN_REQUEST_TOKEN=pull-request VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The job runs for a pull request, so its token doesn't satisfy the write policy and the backend rejects the upload. The task succeeds.

```
$ vtt write-file dist/output.txt built

---
vt run: remote-cache#build not uploaded to the remote cache: HTTP status 403. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[remote-cache] POST /store 403
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
      ⚠ Not uploaded to the remote cache: HTTP status 403
        ↳ Write not permitted
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `remote-cache-server stop`

```
```

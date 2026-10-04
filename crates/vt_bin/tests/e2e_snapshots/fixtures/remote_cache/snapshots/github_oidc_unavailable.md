# github_oidc_unavailable

## `VP_REMOTE_CACHE=read-write VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test ACTIONS_ID_TOKEN_REQUEST_URL=http://127.0.0.1:0/token?api-version=2.0 ACTIONS_ID_TOKEN_REQUEST_TOKEN=request-token VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 vt run build`

The job can request GitHub Actions OIDC tokens, so the upload needs one first. Nothing can listen on port 0, so the token request fails, and the upload fails without being sent. The task succeeds.

```
$ vtt write-file dist/output.txt built ○ cache miss: remote cache fetch failed, executing

---
vt run: remote-cache#build not uploaded to the remote cache: failed to authenticate. (Run `vt run --last-details` for full details)
```

## `vt run --last-details`

The details include why the token request failed.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Cache miss: remote cache fetch failed
        ↳ network error
        ↳ error sending request
        ↳ client error (Connect)
        ↳ tcp connect error
        ↳ <os error>
      ⚠ Not uploaded to the remote cache: failed to authenticate
        ↳ GitHub Actions OIDC token request failed
        ↳ error sending request
        ↳ client error (Connect)
        ↳ tcp connect error
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

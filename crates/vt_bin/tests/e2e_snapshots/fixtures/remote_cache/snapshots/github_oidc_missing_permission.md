# github_oidc_missing_permission

## `VP_REMOTE_CACHE=read-write GITHUB_ACTIONS=true vtt oidc-remote-cache --without-oidc vt run verify`

Without the OIDC variables, the first upload has no token and is rejected. The second upload is skipped without a request, with the same reason. The tasks succeed.

```
$ vtt write-file dist/output.txt built

$ vtt print verified
verified

---
vt run: 0/2 cache hit (0%). remote-cache#build (and 1 more) not uploaded to the remote cache: HTTP status 401. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 404
[remote-cache] POST /store 401
[remote-cache] POST /fetch 404
```

## `vt run --last-details`

The details suggest granting `id-token: write`.

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
      ⚠ Not uploaded to the remote cache: HTTP status 401
        ↳ grant `id-token: write` to this job
  ·······················································
  [2] remote-cache#verify: $ vtt print verified ✓
      → Cache miss: no previous cache entry found
      ⚠ Not uploaded to the remote cache: HTTP status 401
        ↳ grant `id-token: write` to this job
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

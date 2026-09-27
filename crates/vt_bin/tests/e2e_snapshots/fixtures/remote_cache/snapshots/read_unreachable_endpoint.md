# read_unreachable_endpoint

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vt run build`

Nothing can listen on port 0. The failed fetch is the miss reason. Read failures aren't warnings.

```
$ vtt write-file dist/output.txt built ○ cache miss: remote cache fetch failed, executing
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
      → Cache miss: remote cache fetch failed
        ↳ network error
        ↳ error sending request
        ↳ client error (Connect)
        ↳ tcp connect error
        ↳ <os error>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

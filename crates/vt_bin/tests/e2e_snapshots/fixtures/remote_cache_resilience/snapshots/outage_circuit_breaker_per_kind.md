# outage_circuit_breaker_per_kind

Three consecutive exhausted read failures open the read circuit breaker without blocking uploads, and successful uploads do not reset read failures.

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 VP_RUN_INTERNAL_REMOTE_CACHE_NO_BACKOFF=1 remote-cache-server run --github-actions --always-fail /fetch=503 --summary vt run step4`

step1..step3 exhaust 3 fetch attempts each (9 total) and open the read breaker. step4 skips the fetch request, while all 4 stores succeed.

```
$ vtt print step1 ○ cache miss: remote cache fetch failed, executing
step1

$ vtt print step2 ○ cache miss: remote cache fetch failed, executing
step2

$ vtt print step3 ○ cache miss: remote cache fetch failed, executing
step3

$ vtt print step4 ○ cache miss: remote cache temporarily unavailable, executing
step4

---
vt run: 0/4 cache hit (0%), 4/4 successful. (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 503 x9
[remote-cache] POST /store 200 x4
```

## `vt run --last-details`

step1..step3 report HTTP status 503; step4 reports remote cache temporarily unavailable; no upload warnings.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   4 tasks • 0 cache hits • 4 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache-resilience#step1: $ vtt print step1 ✓
      → Cache miss: remote cache fetch failed
        ↳ HTTP status 503
        ↳ busy
  ·······················································
  [2] remote-cache-resilience#step2: $ vtt print step2 ✓
      → Cache miss: remote cache fetch failed
        ↳ HTTP status 503
        ↳ busy
  ·······················································
  [3] remote-cache-resilience#step3: $ vtt print step3 ✓
      → Cache miss: remote cache fetch failed
        ↳ HTTP status 503
        ↳ busy
  ·······················································
  [4] remote-cache-resilience#step4: $ vtt print step4 ✓
      → Cache miss: remote cache temporarily unavailable
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## `remote-cache-server stop`

```
```

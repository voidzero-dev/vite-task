# disable_cache_noop_allows_cache_hit

Exercises the temporary `disableCache` no-op workaround. The tool asks the
runner not to cache this run, but the client ignores that request, so the next
invocation hits the cache.

## `vt run disable-cache`

first run — tool calls disableCache, currently ignored by the client

```
$ node scripts/disable_cache.mjs
```

## `vt run disable-cache`

cache hit because disableCache is temporarily a no-op

```
$ node scripts/disable_cache.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vt run --last-details`

summary reports the replayed cache hit

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 1 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] ipc-client-test#disable-cache: $ node scripts/disable_cache.mjs ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

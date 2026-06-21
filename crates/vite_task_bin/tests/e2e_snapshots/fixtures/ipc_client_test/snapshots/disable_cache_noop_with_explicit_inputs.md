# disable_cache_noop_with_explicit_inputs

Exercises the temporary `disableCache` no-op workaround on a cached task with
explicit inputs. The client ignores the opt-out request, so the second run hits
even when fspy auto-input inference is disabled.

## `vt run disable-cache-explicit-input`

first run uses input: [] and calls disableCache, currently ignored by the client

```
$ node scripts/disable_cache.mjs
```

## `vt run disable-cache-explicit-input`

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
  [1] ipc-client-test#disable-cache-explicit-input: $ node scripts/disable_cache.mjs ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

# disable_cache_forces_reexecution

Exercises `disableCache`. The tool asks the runner not to cache this run,
so the next invocation re-executes instead of hitting a prior entry.

## `vt run disable-cache`

first run — tool calls disableCache

```
$ node scripts/disable_cache.mjs
```

## `vt run disable-cache`

cache miss (NotFound) because nothing was cached

```
$ node scripts/disable_cache.mjs
```

## `vt run --last-details`

summary names the opt-out as the not-cached reason

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] ipc-client-test#disable-cache: $ node scripts/disable_cache.mjs ✓
      → Not cached: the task opted out of caching
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

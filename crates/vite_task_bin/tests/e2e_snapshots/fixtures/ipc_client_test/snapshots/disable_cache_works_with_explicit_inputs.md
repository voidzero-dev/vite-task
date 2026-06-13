# disable_cache_works_with_explicit_inputs

Exercises `disableCache` on a cached task with explicit inputs. The runner must still inject IPC even when fspy auto-input inference is disabled, or the tool's cache opt-out becomes a no-op and the second run incorrectly hits.

## `vt run disable-cache-explicit-input`

first run uses input: [] and asks the runner not to cache

```
$ node scripts/disable_cache.mjs
```

## `vt run disable-cache-explicit-input`

re-executes because the first run was not cached

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
  [1] ipc-client-test#disable-cache-explicit-input: $ node scripts/disable_cache.mjs ✓
      → Not cached: the task opted out of caching
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

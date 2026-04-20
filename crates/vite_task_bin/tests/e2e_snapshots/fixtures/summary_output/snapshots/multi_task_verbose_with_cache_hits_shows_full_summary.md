# multi_task_verbose_with_cache_hits_shows_full_summary

With `-v` on a multi-task re-run, the full summary should report per-task
cache hits (not just overall stats).

## `vt run -r build`

first run, populate cache

```
~/packages/a$ vtt print built-a
built-a

~/packages/b$ vtt print built-b
built-b

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run -r -v build`

second run, verbose with cache hits

```
~/packages/a$ vtt print built-a ◉ cache hit, replaying
built-a

~/packages/b$ vtt print built-b ◉ cache hit, replaying
built-b


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 2 cache hits • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] @summary/a#build: ~/packages/a$ vtt print built-a ✓
      → Cache hit - output replayed -
  ·······················································
  [2] @summary/b#build: ~/packages/b$ vtt print built-b ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

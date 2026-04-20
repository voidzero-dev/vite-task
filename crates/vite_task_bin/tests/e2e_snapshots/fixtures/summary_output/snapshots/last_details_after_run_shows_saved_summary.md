# last_details_after_run_shows_saved_summary

After a single-task run, `vt run --last-details` should display the saved
summary from disk.

## `vt run build`

populate summary file

```
~/packages/a$ vtt print built-a
built-a
```

## `vt run --last-details`

display saved summary

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] @summary/a#build: ~/packages/a$ vtt print built-a ✓
      → Cache miss: no previous cache entry found
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

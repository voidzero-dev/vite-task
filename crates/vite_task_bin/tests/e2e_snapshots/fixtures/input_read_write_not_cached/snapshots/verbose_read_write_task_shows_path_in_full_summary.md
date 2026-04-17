# verbose_read_write_task_shows_path_in_full_summary

Tests that tasks modifying their own inputs (read-write overlap) are not cached.
vtt replace-file-content reads then writes the same file — fspy detects both.

## `vt run -v task`

```
~/packages/rw-pkg$ vtt replace-file-content src/data.txt i !


━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 tasks • 0 cache hits • 1 cache misses
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] @test/rw-pkg#task: ~/packages/rw-pkg$ vtt replace-file-content src/data.txt i ! ✓
      → Not cached: read and wrote 'packages/rw-pkg/src/data.txt'
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

# verbose_read_write_task_shows_path_in_full_summary

Under `-v`, the full summary should list the exact overlapping path that
caused the task to skip caching.

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

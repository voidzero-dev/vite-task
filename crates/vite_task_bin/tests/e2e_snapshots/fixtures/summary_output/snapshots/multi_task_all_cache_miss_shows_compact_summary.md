# multi_task_all_cache_miss_shows_compact_summary

A multi-task (`-r`) run where every task misses should still print a compact
summary with 0 hits.

## `vt run -r build`

```
~/packages/a$ vtt print built-a
built-a

~/packages/b$ vtt print built-b
built-b

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

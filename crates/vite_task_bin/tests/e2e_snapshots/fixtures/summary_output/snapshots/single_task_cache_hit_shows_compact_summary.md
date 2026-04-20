# single_task_cache_hit_shows_compact_summary

When a single task hits the cache on a re-run, the compact one-line summary
should be emitted.

## `vt run build`

first run, cache miss

```
~/packages/a$ vtt print built-a
built-a
```

## `vt run build`

second run, cache hit → compact summary

```
~/packages/a$ vtt print built-a ◉ cache hit, replaying
built-a

---
vt run: cache hit.
```

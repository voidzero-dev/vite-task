# single_task_cache_hit_shows_compact_summary

Tests for compact and verbose summary output

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

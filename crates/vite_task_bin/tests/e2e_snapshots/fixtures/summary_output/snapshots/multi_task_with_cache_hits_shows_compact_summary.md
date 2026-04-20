# multi_task_with_cache_hits_shows_compact_summary

On the second multi-task (`-r`) run, the compact summary should report the
correct hit count.

## `vt run -r build`

first run, all miss

```
~/packages/a$ vtt print built-a
built-a

~/packages/b$ vtt print built-b
built-b

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run -r build`

second run, all hit

```
~/packages/a$ vtt print built-a ◉ cache hit, replaying
built-a

~/packages/b$ vtt print built-b ◉ cache hit, replaying
built-b

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

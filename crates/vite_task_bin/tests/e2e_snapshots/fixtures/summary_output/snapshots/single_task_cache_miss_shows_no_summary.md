# single_task_cache_miss_shows_no_summary

A single task on its first run (cache miss) should produce no summary line
at all — only the task's own output.

## `vt run build`

```
~/packages/a$ vtt print built-a
built-a
```

# single_task_silent_suppresses_command_line_and_summary

`vt run --silent` on a cache hit should print only the task's own output — no command line, no cache-hit indicator, and no summary.

## `vt run build`

first run, populate cache

```
~/packages/a$ vtt print built-a
built-a
```

## `vt run --silent build`

second run, cache hit → only task output

```
built-a
```

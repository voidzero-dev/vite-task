# single_task__cache_hit__replayed

A cache-hit second run should replay the previously captured output verbatim
instead of re-executing the task.

## `vt run check-tty-cached`

```
$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

## `vt run check-tty-cached`

```
$ vtt check-tty ◉ cache hit, replaying
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: cache hit.
```

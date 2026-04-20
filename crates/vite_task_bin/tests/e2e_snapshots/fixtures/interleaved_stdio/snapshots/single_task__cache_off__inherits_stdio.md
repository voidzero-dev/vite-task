# single_task__cache_off__inherits_stdio

In interleaved mode with caching off, a single task should inherit stdio from
the parent — all fds should be TTYs.

## `vt run check-tty`

```
$ vtt check-tty ⊘ cache disabled
stdin:tty
stdout:tty
stderr:tty
```

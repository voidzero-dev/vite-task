# single_task__cache_miss__piped_stdio

On a cache miss in interleaved mode, stdio must be piped (not a TTY) so that
output can be captured for future replay.

## `vt run check-tty-cached`

```
$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

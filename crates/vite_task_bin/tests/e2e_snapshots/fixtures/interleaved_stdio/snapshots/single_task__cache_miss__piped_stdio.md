# single_task__cache_miss__piped_stdio

Tests stdio behavior in interleaved mode (default --log mode).

In interleaved mode:
- cache off  → all stdio inherited (stdin from parent, stdout/stderr to terminal)
- cache on   → stdin is /dev/null, stdout/stderr are piped (for capture/replay)

This applies identically regardless of task count.

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run check-tty-cached`

```
$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

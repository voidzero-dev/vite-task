# multiple_tasks__cache_hit__replayed

Tests stdio behavior in interleaved mode (default --log mode).

In interleaved mode:
- cache off  → all stdio inherited (stdin from parent, stdout/stderr to terminal)
- cache on   → stdin is /dev/null, stdout/stderr are piped (for capture/replay)

This applies identically regardless of task count.

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run -r check-tty-cached`

```
~/packages/other$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run -r check-tty-cached`

```
~/packages/other$ vtt check-tty ◉ cache hit, replaying
stdin:not-tty
stdout:not-tty
stderr:not-tty

$ vtt check-tty ◉ cache hit, replaying
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

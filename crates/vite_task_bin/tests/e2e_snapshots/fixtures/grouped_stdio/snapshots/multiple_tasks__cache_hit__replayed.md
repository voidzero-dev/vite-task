# multiple_tasks__cache_hit__replayed

Tests stdio behavior in grouped mode (--log=grouped).

In grouped mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are buffered per task and printed as a block on completion

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run --log=grouped -r check-tty-cached`

```
[other#check-tty-cached] ~/packages/other$ vtt check-tty
── [other#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

[grouped-stdio-test#check-tty-cached] $ vtt check-tty
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vt run --log=grouped -r check-tty-cached`

```
[other#check-tty-cached] ~/packages/other$ vtt check-tty ◉ cache hit, replaying
── [other#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

[grouped-stdio-test#check-tty-cached] $ vtt check-tty ◉ cache hit, replaying
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

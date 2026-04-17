# single_task__cache_hit__replayed

Tests stdio behavior in labeled mode (--log=labeled).

In labeled mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are piped through a line-prefixing writer ([pkg#task])

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run --log=labeled check-tty-cached`

```
[labeled-stdio-test#check-tty-cached] $ vtt check-tty
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty
```

## `vt run --log=labeled check-tty-cached`

```
[labeled-stdio-test#check-tty-cached] $ vtt check-tty ◉ cache hit, replaying
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty

---
vt run: cache hit.
```

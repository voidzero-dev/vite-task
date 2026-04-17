# multiple_tasks__cache_miss__piped_stdio

Tests stdio behavior in labeled mode (--log=labeled).

In labeled mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are piped through a line-prefixing writer ([pkg#task])

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run --log=labeled -r check-tty-cached`

```
[other#check-tty-cached] ~/packages/other$ vtt check-tty
[other#check-tty-cached] stdin:not-tty
[other#check-tty-cached] stdout:not-tty
[other#check-tty-cached] stderr:not-tty

[labeled-stdio-test#check-tty-cached] $ vtt check-tty
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

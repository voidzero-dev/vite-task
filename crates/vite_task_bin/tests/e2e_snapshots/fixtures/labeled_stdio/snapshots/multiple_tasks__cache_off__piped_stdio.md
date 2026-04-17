# multiple_tasks__cache_off__piped_stdio

Tests stdio behavior in labeled mode (--log=labeled).

In labeled mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are piped through a line-prefixing writer ([pkg#task])

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run --log=labeled -r check-tty`

```
[other#check-tty] ~/packages/other$ vtt check-tty
[other#check-tty] stdin:not-tty
[other#check-tty] stdout:not-tty
[other#check-tty] stderr:not-tty

[labeled-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
[labeled-stdio-test#check-tty] stdin:not-tty
[labeled-stdio-test#check-tty] stdout:not-tty
[labeled-stdio-test#check-tty] stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

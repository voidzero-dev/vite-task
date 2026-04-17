# stdin_is_always_null

Tests stdio behavior in labeled mode (--log=labeled).

In labeled mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are piped through a line-prefixing writer ([pkg#task])

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vtt pipe-stdin from-stdin -- vt run --log=labeled read-stdin`

```
[labeled-stdio-test#read-stdin] $ vtt read-stdin ⊘ cache disabled
```

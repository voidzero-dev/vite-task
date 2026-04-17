# stdin_is_always_null

Tests stdio behavior in grouped mode (--log=grouped).

In grouped mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are buffered per task and printed as a block on completion

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vtt pipe-stdin from-stdin -- vt run --log=grouped read-stdin`

```
[grouped-stdio-test#read-stdin] $ vtt read-stdin ⊘ cache disabled
```

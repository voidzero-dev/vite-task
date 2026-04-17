# single_task__cache_off__grouped_output

Tests stdio behavior in grouped mode (--log=grouped).

In grouped mode, stdio is always piped regardless of cache state:
- stdin is /dev/null
- stdout/stderr are buffered per task and printed as a block on completion

`check-tty` prints whether each stdio fd is a TTY.
`read-stdin` reads one line from stdin and prints it.

## `vt run --log=grouped check-tty`

```
[grouped-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
── [grouped-stdio-test#check-tty] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

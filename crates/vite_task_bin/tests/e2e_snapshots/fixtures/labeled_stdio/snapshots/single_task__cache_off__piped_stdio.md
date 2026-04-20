# single_task__cache_off__piped_stdio

Under `--log=labeled` with caching off, a single task's stdio should be piped
through the line-prefixing writer (not a TTY).

## `vt run --log=labeled check-tty`

```
[labeled-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
[labeled-stdio-test#check-tty] stdin:not-tty
[labeled-stdio-test#check-tty] stdout:not-tty
[labeled-stdio-test#check-tty] stderr:not-tty
```

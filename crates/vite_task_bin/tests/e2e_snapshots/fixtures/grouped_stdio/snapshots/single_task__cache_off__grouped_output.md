# single_task__cache_off__grouped_output

Under `--log=grouped` with caching off, a single task's piped stdout/stderr
should be printed as one grouped block; none of the fds should be TTYs.

## `vt run --log=grouped check-tty`

```
[grouped-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
── [grouped-stdio-test#check-tty] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

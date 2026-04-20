# single_task__cache_miss__grouped_output

On a cache miss, grouped mode should still pipe stdio and emit one block for
the single task.

## `vt run --log=grouped check-tty-cached`

```
[grouped-stdio-test#check-tty-cached] $ vtt check-tty
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

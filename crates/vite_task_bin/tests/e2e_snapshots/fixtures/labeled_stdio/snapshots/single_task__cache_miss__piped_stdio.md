# single_task__cache_miss__piped_stdio

On a cache miss for a single task, labeled mode should still pipe stdio and
prefix output lines.

## `vt run --log=labeled check-tty-cached`

```
[labeled-stdio-test#check-tty-cached] $ vtt check-tty
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty
```

# multiple_tasks__cache_miss__piped_stdio

On a cache miss across multiple tasks, each task's piped output should be
individually prefixed in labeled mode.

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

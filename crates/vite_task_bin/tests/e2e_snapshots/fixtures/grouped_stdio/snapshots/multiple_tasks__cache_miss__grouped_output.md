# multiple_tasks__cache_miss__grouped_output

On a cache miss across multiple tasks, grouped mode should emit one block
per task with piped, non-TTY stdio.

## `vt run --log=grouped -r check-tty-cached`

```
[other#check-tty-cached] ~/packages/other$ vtt check-tty
── [other#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

[grouped-stdio-test#check-tty-cached] $ vtt check-tty
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

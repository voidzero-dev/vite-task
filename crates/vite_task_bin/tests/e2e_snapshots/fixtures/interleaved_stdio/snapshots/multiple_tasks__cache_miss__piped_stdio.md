# multiple_tasks__cache_miss__piped_stdio

## `vt run -r check-tty-cached`

```
~/packages/other$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

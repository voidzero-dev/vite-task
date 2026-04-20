# multiple_tasks__cache_off__inherit_stdio

In interleaved mode with caching off, multiple tasks under `-r` should each
inherit stdio (TTY-preserving behavior applies per task regardless of count).

## `vt run -r check-tty`

```
~/packages/other$ vtt check-tty
stdin:not-tty
stdout:not-tty
stderr:not-tty

$ vtt check-tty ⊘ cache disabled
stdin:tty
stdout:tty
stderr:tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

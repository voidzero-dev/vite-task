# single_task__cache_hit__replayed

A cache-hit replay of a single task under grouped mode should reproduce the
original grouped block from cached output.

## `vt run --log=grouped check-tty-cached`

```
[grouped-stdio-test#check-tty-cached] $ vtt check-tty
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty
```

## `vt run --log=grouped check-tty-cached`

```
[grouped-stdio-test#check-tty-cached] $ vtt check-tty ◉ cache hit, replaying
── [grouped-stdio-test#check-tty-cached] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: cache hit.
```

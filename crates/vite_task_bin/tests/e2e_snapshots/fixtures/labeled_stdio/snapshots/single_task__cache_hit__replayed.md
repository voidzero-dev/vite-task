# single_task__cache_hit__replayed

A cache-hit replay of a single task under labeled mode should reproduce the
labeled output from cached data.

## `vt run --log=labeled check-tty-cached`

```
[labeled-stdio-test#check-tty-cached] $ vtt check-tty
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty
```

## `vt run --log=labeled check-tty-cached`

```
[labeled-stdio-test#check-tty-cached] $ vtt check-tty ◉ cache hit, replaying
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty

---
vt run: cache hit.
```

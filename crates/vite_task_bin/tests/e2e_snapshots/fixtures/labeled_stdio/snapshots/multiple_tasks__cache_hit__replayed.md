# multiple_tasks__cache_hit__replayed

A cache-hit replay across multiple tasks under labeled mode should reproduce
each task's labeled output.

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

## `vt run --log=labeled -r check-tty-cached`

```
[other#check-tty-cached] ~/packages/other$ vtt check-tty ◉ cache hit, replaying
[other#check-tty-cached] stdin:not-tty
[other#check-tty-cached] stdout:not-tty
[other#check-tty-cached] stderr:not-tty

[labeled-stdio-test#check-tty-cached] $ vtt check-tty ◉ cache hit, replaying
[labeled-stdio-test#check-tty-cached] stdin:not-tty
[labeled-stdio-test#check-tty-cached] stdout:not-tty
[labeled-stdio-test#check-tty-cached] stderr:not-tty

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

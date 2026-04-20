# multiple_tasks__cache_off__piped_stdio

Under `--log=labeled` with caching off, multiple tasks should each get piped
stdio and each line should be prefixed with `[pkg#task]`.

## `vt run --log=labeled -r check-tty`

```
[other#check-tty] ~/packages/other$ vtt check-tty
[other#check-tty] stdin:not-tty
[other#check-tty] stdout:not-tty
[other#check-tty] stderr:not-tty

[labeled-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
[labeled-stdio-test#check-tty] stdin:not-tty
[labeled-stdio-test#check-tty] stdout:not-tty
[labeled-stdio-test#check-tty] stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

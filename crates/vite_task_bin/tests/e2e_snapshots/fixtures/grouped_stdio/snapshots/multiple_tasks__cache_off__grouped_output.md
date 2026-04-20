# multiple_tasks__cache_off__grouped_output

Under `--log=grouped` with caching off, each task's output should be emitted
as its own grouped block (one block per task, never interleaved).

## `vt run --log=grouped -r check-tty`

```
[other#check-tty] ~/packages/other$ vtt check-tty
── [other#check-tty] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

[grouped-stdio-test#check-tty] $ vtt check-tty ⊘ cache disabled
── [grouped-stdio-test#check-tty] ──
stdin:not-tty
stdout:not-tty
stderr:not-tty

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

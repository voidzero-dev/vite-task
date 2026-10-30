# ctrl_c_prevents_future_tasks_with___continue

`--continue` doesn't stop Ctrl+C from cancelling the run: a exits 0 after Ctrl+C, and with `--continue` no task's exit cancels anything, so only Ctrl+C keeps b, which depends on a, from starting.

## `vt run -r --continue --no-cache dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
ctrl-c received
```

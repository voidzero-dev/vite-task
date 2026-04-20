# ctrl_c_prevents_future_tasks

Ctrl+C while running sequentially (b depends on a) should terminate a and
prevent b from being scheduled.

## `vt run -r --no-cache dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
ctrl-c received
```

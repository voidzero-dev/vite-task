# ctrl_c_prevents_future_tasks

Tests that Ctrl+C (SIGINT) propagates to and terminates a running task.

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

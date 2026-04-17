# ctrl_c_terminates_running_tasks

Tests that Ctrl+C (SIGINT) propagates to and terminates a running task.

## `vt run --no-cache @ctrl-c/a#dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc ⊘ cache disabled
ctrl-c received
```

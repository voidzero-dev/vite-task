# ctrl_c_terminates_running_tasks

Ctrl+C (SIGINT) on an uncached run should propagate to the running task and terminate it.

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

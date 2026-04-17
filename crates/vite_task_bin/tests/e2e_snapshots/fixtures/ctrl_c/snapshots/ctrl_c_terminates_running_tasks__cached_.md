# ctrl_c_terminates_running_tasks__cached_

Tests that Ctrl+C (SIGINT) propagates to and terminates a running task.

## `vt run @ctrl-c/a#dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-ctrlc
```

**← write-key:** `ctrl-c`

```
~/packages/a$ vtt exit-on-ctrlc
ctrl-c received
```

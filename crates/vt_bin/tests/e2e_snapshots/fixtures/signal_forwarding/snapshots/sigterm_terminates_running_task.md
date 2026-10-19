# sigterm_terminates_running_task

SIGTERM sent to the runner alone (as `kill <pid>`, a supervisor, or a container
runtime would) must reach the running task: the task receives SIGTERM, exits,
and the runner waits for it instead of exiting immediately and orphaning it.

## `vt run --no-cache @signal-forwarding/a#dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-sigterm ⊘ cache disabled
```

**← signal:** `sigterm`

```
~/packages/a$ vtt exit-on-sigterm ⊘ cache disabled
sigterm received
```

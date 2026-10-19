# sigterm_terminates_running_task__cached_

Same as above, but under the cached execution path (piped stdio + fspy tracking).

## `vt run @signal-forwarding/a#dev`

**→ expect-milestone:** `ready`

```
~/packages/a$ vtt exit-on-sigterm
```

**← signal:** `sigterm`

```
~/packages/a$ vtt exit-on-sigterm
sigterm received
```

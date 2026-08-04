# ctrl_c_terminates_running_tasks__cached_

Same as above, but under the cached execution path (piped stdio + fspy tracking).

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

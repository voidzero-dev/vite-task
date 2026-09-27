# ctrl_c_during_fetch

## `vtt stalled-remote-cache vt run build`

The endpoint never responds. Ctrl-C stops the fetch, and the task doesn't start.

**→ expect-milestone:** `request`

```
```

**← write-key:** `ctrl-c`

```
---
vt run: 0/0 cache hit (0%). (Run `vt run --last-details` for full details)
```

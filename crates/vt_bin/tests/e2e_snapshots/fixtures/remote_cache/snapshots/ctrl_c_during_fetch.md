# ctrl_c_during_fetch

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vtt stalled-remote-cache --stall /fetch vt run build`

The proxy never answers the fetch, so nothing needs to listen behind it. Ctrl-C stops the fetch, and the task doesn't start.

**→ expect-milestone:** `stalled`

```
```

**← write-key:** `ctrl-c`

```

```

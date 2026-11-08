# fetch_before_dependencies

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vtt stalled-remote-cache --stall /fetch --milestone vt run test`

The proxy never answers a fetch. test depends on build, so it can only run after build, but both fetches start when the run starts. Ctrl-C stops them, and neither task starts.

**→ expect-milestone:** `stalled:2`

```
```

**← write-key:** `ctrl-c`

```
```

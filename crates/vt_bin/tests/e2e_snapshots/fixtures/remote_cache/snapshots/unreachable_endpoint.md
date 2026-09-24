# unreachable_endpoint

## `VP_REMOTE_CACHE=read-write VP_REMOTE_CACHE_URL=http://127.0.0.1:1/projects/test vt run build`

Nothing listens on port 1. The failed upload is a warning. The task succeeds.

```
$ vtt write-file dist/output.txt built

---
vt run: remote-cache#build not uploaded to the remote cache: network error. (Run `vt run --last-details` for full details)
```

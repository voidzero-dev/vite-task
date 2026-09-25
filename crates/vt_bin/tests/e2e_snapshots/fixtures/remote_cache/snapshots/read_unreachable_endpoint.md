# read_unreachable_endpoint

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vt run build`

Nothing can listen on port 0. The failed fetch is the miss reason. Read failures aren't warnings.

```
$ vtt write-file dist/output.txt built ○ cache miss: remote cache fetch failed (network error), executing
```

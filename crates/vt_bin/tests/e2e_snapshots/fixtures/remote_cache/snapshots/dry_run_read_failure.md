# dry_run_read_failure

## `VP_REMOTE_CACHE_URL=http://127.0.0.1:0/projects/test vt run --dry-run build`

Nothing can listen on port 0. The failed fetch is a miss, as in a real run.

```
$ vtt write-file dist/output.txt built ○ cache miss: remote cache fetch failed
```

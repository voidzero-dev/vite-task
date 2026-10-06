# remote_cache_not_checked

The dry run only reads the local cache. It says so once when a remote cache is configured.

## `VP_REMOTE_CACHE_URL=cache.example/projects/test vt run --dry-run build`

```
dry-run#build: $ vtt print-file src.txt → Cache miss: no previous cache entry found
Remote cache not checked: --dry-run only reads the local cache
```

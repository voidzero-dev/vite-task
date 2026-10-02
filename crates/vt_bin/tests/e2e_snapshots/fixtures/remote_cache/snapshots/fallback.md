# fallback

## `VP_REMOTE_CACHE=read-write REMOTE_CACHE_STORE_DELAY_MS=500 remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt replace-file-content vite-task.json built rebuilt`

```
```

## `remote-cache-server vt run build`

The entry stored for this task has a different key. The miss reason compares it with the current key.

```
$ vtt write-file dist/output.txt rebuilt ○ cache miss: args changed, executing

[remote-cache] POST /fetch 200 fallback
```

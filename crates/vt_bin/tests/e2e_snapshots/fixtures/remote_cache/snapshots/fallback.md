# fallback

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt replace-file-content vite-task.json built rebuilt`

```
```

## `remote-cache-server run vt run build`

The entry stored for this task has a different key. The miss reason compares it with the current key.

```
$ vtt write-file dist/output.txt rebuilt ○ cache miss: args changed, executing

[remote-cache] POST /fetch 200 fallback
```

## `remote-cache-server stop`

```
```

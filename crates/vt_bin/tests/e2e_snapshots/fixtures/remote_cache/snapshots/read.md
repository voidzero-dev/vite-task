# read

## `VP_REMOTE_CACHE=read-write REMOTE_CACHE_STORE_DELAY_MS=500 remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt changed`

```
```

## `remote-cache-server vt run build`

An endpoint without a mode selects read. The task reruns without uploading.

```
$ vtt write-file dist/output.txt built ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact
```

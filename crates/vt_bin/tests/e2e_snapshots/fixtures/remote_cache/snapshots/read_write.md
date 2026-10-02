# read_write

## `VP_REMOTE_CACHE=read-write REMOTE_CACHE_STORE_DELAY_MS=500 remote-cache-server vt run build`

The fetch finds no entry. The new execution is uploaded with one store request, which the server delays. vt run waits for it after the task finishes.

```
$ vtt write-file dist/output.txt built

Waiting for 1 remote cache upload to finish (Ctrl-C to cancel)...
[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

A local hit makes no requests.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

# read_write

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run vt run build`

The fetch finds no entry. The new execution is uploaded with one store request, and vt run waits for it after the task finishes.

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write remote-cache-server run vt run build`

A local hit makes no requests.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `remote-cache-server stop`

```
```

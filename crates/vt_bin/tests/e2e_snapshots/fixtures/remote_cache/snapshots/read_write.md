# read_write

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

A new execution is uploaded with one store request.

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

A local hit makes no requests.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

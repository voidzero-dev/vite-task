# read_write

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

The fetch finds no entry. The new execution is uploaded with one store request.

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 200 not_found
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

A local hit makes no requests.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

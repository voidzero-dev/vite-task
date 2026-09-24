# read

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 200 not_found
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

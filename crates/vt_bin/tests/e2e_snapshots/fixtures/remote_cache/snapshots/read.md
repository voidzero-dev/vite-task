# read

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt changed`

```
```

## `remote-cache-server run vt run build`

An endpoint without a mode selects read. The task reruns without uploading.

```
$ vtt write-file dist/output.txt built ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact
```

## `remote-cache-server stop`

```
```

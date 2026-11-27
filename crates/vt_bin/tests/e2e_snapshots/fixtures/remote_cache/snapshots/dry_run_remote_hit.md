# dry_run_remote_hit

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

## `remote-cache-server run vt run --dry-run build`

A dry run reads the remote cache, as a real run does, but doesn't download the hit's archive: there's no `GET /blob` request.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit

---
vt run: remote cache hit, nothing was run.
[remote-cache] POST /fetch 200 exact [1]
```

## `remote-cache-server stop`

```
```

# entries_per_key

The namespace keeps two entries per key.

## `remote-cache-server start --entries-per-key 2`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt cp src/a.txt output.txt

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 2`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [1]
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 3`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The third entry retires the oldest one.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [2, 1]
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 1`

```
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run build`

The entry for 1 was retired, so the run misses.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [3, 2]
```

## `vtt write-file src/a.txt 2`

```
```

## `remote-cache-server run vt run build`

The entry for 2 is still kept.

```
$ vtt cp src/a.txt output.txt ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact [3, 2]
[remote-cache] GET /blob/2 200
```

## `remote-cache-server stop`

```
```

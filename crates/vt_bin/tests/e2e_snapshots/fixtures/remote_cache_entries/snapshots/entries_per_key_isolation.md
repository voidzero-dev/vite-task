# entries_per_key_isolation

The namespace keeps one entry per key. Tasks with other values of `MODE` have other keys.

## `remote-cache-server start --entries-per-key 1`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 MODE=a remote-cache-server run --github-actions vt run build`

The entry for `MODE` a and `src/a.txt` with 1 has blob 1.

```
$ vtt cp src/a.txt output.txt

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 MODE=b remote-cache-server run --github-actions vt run build`

Key b has no entry yet, so the fetch falls back to key a.

```
$ vtt cp src/a.txt output.txt ○ cache miss: env 'MODE' changed, executing

[remote-cache] POST /fetch 200 fallback
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 2`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 MODE=b remote-cache-server run --github-actions vt run build`

The entry for 2 replaces key b's entry for 1.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [2]
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 1`

```
```

## `vt cache clean`

```
```

## `MODE=b remote-cache-server run vt run build`

Key b only has its entry for 2, so the run misses.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [3]
```

## `MODE=a remote-cache-server run vt run build`

Key a still has its entry for 1: entries of other keys don't take its place.

```
$ vtt cp src/a.txt output.txt ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact [1]
[remote-cache] GET /blob/1 200
```

## `remote-cache-server stop`

```
```

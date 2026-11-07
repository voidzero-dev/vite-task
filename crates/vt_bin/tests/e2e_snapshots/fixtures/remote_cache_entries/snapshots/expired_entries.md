# expired_entries

## `remote-cache-server start`

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

## `remote-cache-server expire-entry 2`

```
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run build`

The expired entry isn't fetched, and the miss is explained against the newest entry that hasn't expired.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [1]
```

## `vtt write-file src/a.txt 1`

```
```

## `remote-cache-server run vt run build`

The entry for 1 hasn't expired.

```
$ vtt cp src/a.txt output.txt ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact [1]
[remote-cache] GET /blob/1 200
```

## `remote-cache-server expire-entry 1`

```
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run build`

Once every entry of the key has expired, the fetch finds nothing, not even a fallback.

```
$ vtt cp src/a.txt output.txt

[remote-cache] POST /fetch 404
```

## `remote-cache-server stop`

```
```

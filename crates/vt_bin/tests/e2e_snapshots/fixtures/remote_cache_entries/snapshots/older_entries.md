# older_entries

A key keeps its newest entries. A lookup checks them newest first and restores the first one that matches.

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The first entry, for `src/a.txt` with 1, has blob 1.

```
$ vtt cp src/a.txt output.txt

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 2`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The fetch returns the key's only entry, which doesn't match. The entry for 2 is added.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [1]
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 3`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The entries come newest first.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact [2, 1]
[remote-cache] POST /store 200
```

## `vtt write-file src/a.txt 1`

```
```

## `remote-cache-server run vt run build`

The local entry is for 3. Of the remote entries, only the oldest matches, so its blob is downloaded.

```
$ vtt cp src/a.txt output.txt ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact [3, 2, 1]
[remote-cache] GET /blob/1 200
```

## `vtt print-file output.txt`

The outputs of the run for 1 are restored.

```
1
```

## `remote-cache-server stop`

```
```

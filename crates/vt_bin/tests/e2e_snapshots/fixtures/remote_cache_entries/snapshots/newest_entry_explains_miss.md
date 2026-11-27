# newest_entry_explains_miss

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt cp src/a.txt output.txt

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt write-file src/b.txt x`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

The entry for `src/b.txt` with x is added.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'b.txt' added in 'src', executing

[remote-cache] POST /fetch 200 exact [1]
[remote-cache] POST /store 200
```

## `vtt write-file src/b.txt y`

```
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run build`

No entry matches. The miss is explained against the newest entry, in which `src/b.txt` had other contents. The older entry had no `src/b.txt`.

```
$ vtt cp src/a.txt output.txt ○ cache miss: 'src/b.txt' modified, executing

[remote-cache] POST /fetch 200 exact [2, 1]
```

## `remote-cache-server stop`

```
```

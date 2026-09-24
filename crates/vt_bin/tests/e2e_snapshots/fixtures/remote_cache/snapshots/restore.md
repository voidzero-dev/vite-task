# restore

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 200 not_found
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt rm -rf dist`

```
```

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

A remote hit downloads the output archive. Hits never upload.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

## `vtt print-file dist/output.txt`

The outputs are restored.

```
built
```

## `remote-cache-server vt run build`

The remote hit was recorded locally, so this is a local hit with no requests.

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vt cache clean`

```
```

## `vtt write-file src/a.txt changed`

```
```

## `remote-cache-server vt run build`

The exact entry fails validation, so the archive isn't downloaded.

```
$ vtt write-file dist/output.txt built ○ cache miss: 'src/a.txt' modified, executing

[remote-cache] POST /fetch 200 exact
```

# platform_independent

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run generate`

```
$ vtt write-file dist/generated.txt generated

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vtt rm -rf dist`

```
```

## `remote-cache-server run vt run generate`

The entry stored under the keys that name no platform is a remote hit.

```
$ vtt write-file dist/generated.txt generated ◉ remote cache hit, replaying

---
vt run: remote cache hit.
[remote-cache] POST /fetch 200 exact [1]
[remote-cache] GET /blob/1 200
```

## `vtt print-file dist/generated.txt`

The outputs are restored.

```
generated
```

## `vtt replace-file-content vite-task.json '"platform": []' '"platform": ["os", "arch"]'`

```
```

## `remote-cache-server run vt run generate`

The local cache doesn't depend on the setting, so this is a local hit with no requests.

```
$ vtt write-file dist/generated.txt generated ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run generate`

The remote keys name this platform now, so neither matches the stored entry's keys.

```
$ vtt write-file dist/generated.txt generated

[remote-cache] POST /fetch 404
```

## `remote-cache-server stop`

```
```

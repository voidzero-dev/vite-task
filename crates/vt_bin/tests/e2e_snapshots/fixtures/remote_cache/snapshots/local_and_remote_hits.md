# local_and_remote_hits

## `VP_REMOTE_CACHE=read-write remote-cache-server vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 200 not_found
[remote-cache] POST /store 200
```

## `vt cache clean`

```
```

## `vt run check`

Without an endpoint, check is cached only locally.

```
$ vtt print checked
checked
```

## `remote-cache-server vt run all`

build is a remote hit, and check is a local hit. The summary counts the remote hit.

```
$ vtt write-file dist/output.txt built ◉ remote cache hit, replaying

$ vtt print checked ◉ cache hit, replaying
checked

---
vt run: 2/2 cache hit (100%, 1 remote). (Run `vt run --last-details` for full details)
[remote-cache] POST /fetch 200 exact
[remote-cache] GET /blob/1 200
```

## `vt run --last-details`

The details show which hit came from the remote cache.

```

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   2 tasks • 2 cache hits (1 remote) • 0 cache misses
Performance:  100% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt built ✓
      → Remote cache hit - output replayed -
  ·······················································
  [2] remote-cache#check: $ vtt print checked ✓
      → Cache hit - output replayed -
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

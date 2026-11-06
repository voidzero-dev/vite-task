# fallback_compares_the_cached_run

## `remote-cache-server start`

```
```

## `VP_REMOTE_CACHE=read-write VP_RUN_INTERNAL_HIDE_PENDING_UPLOADS=1 remote-cache-server run --github-actions vt run build`

```
$ vtt write-file dist/output.txt built

[remote-cache] POST /fetch 404
[remote-cache] POST /store 200
```

## `vtt replace-file-content vite-task.json built rebuilt`

```
```

## `vtt write-file src/a.txt changed`

```
```

## `remote-cache-server run vt run --dry-run -v build`

The local cache has the previous run, and its miss reason wins, so the remote fallback's entry isn't fetched.

```
$ vtt write-file dist/output.txt rebuilt ○ cache miss: args changed

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt rebuilt
      → Cache miss: args changed
                    'src/a.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[remote-cache] POST /fetch 200 fallback
```

## `vt cache clean`

```
```

## `remote-cache-server run vt run --dry-run -v build`

The fallback is only a key, so its entry is fetched too, and the miss lists its changed file.

```
$ vtt write-file dist/output.txt rebuilt ○ cache miss: args changed

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
    Vite+ Task Runner • Execution Summary
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Statistics:   1 task • 0 cache hits • 1 cache miss
Performance:  0% cache hit rate

Task Details:
────────────────────────────────────────────────
  [1] remote-cache#build: $ vtt write-file dist/output.txt rebuilt
      → Cache miss: args changed
                    'src/a.txt' modified
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[remote-cache] POST /fetch 200 fallback
[remote-cache] POST /fetch 200 exact [1]
```

## `remote-cache-server stop`

```
```

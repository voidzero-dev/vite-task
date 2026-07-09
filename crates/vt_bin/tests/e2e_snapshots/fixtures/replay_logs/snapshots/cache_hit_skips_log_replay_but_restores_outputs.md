# cache_hit_skips_log_replay_but_restores_outputs

`replayLogs: false` should keep cache hits and output restoration, while suppressing cached stdout/stderr replay.

## `vt run build`

first run — cache miss, prints logs and writes dist/output.txt

```
$ vtt print fresh logs; vtt write-file dist/output.txt artifact
fresh logs
```

## `vtt rm -rf dist`

delete dist/ to prove cache-hit output restoration still happens

```
```

## `vt run build`

second run — cache hit, skips cached log replay

```
$ vtt print fresh logs; vtt write-file dist/output.txt artifact ◉ cache hit

---
vt run: cache hit.
```

## `vtt print-file dist/output.txt`

output file restored from cache

```
artifact
```

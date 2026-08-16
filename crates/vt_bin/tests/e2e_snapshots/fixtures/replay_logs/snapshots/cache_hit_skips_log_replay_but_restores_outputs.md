# cache_hit_skips_log_replay_but_restores_outputs

`replayLogs: false` should keep cache hits and output restoration, while suppressing cached stdout/stderr replay.

## `vt run build`

first run — cache miss, prints logs and writes dist/output.txt

```
$ vtt print fresh logs
fresh logs

$ vtt write-file dist/output.txt artifact

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt rm -rf dist`

delete dist/ to prove cache-hit output restoration still happens

```
```

## `vt run build`

second run — cache hit, skips cached log replay

```
$ vtt print fresh logs ◉ cache hit

$ vtt write-file dist/output.txt artifact ◉ cache hit

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/output.txt`

output file restored from cache

```
artifact
```

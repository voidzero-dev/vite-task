# output_config_change_invalidates_cache

Changing a task's output configuration invalidates the cache entry —
the next run is a miss, not a hit of the stale archive.

## `vt run output-config-change`

first run populates the cache

```
$ vtt write-file dist/out.txt built
```

## `vtt replace-file-content vite-task.json REPLACE_ME dist/**`

change the output globs in the task config

```
```

## `vt run output-config-change`

cache miss: output config changed

```
$ vtt write-file dist/out.txt built ○ cache miss: output configuration changed, executing
```

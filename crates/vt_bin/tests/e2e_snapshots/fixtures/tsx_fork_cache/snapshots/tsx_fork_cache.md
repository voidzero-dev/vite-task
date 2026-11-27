# tsx_fork_cache

`tsx` executes `src/parent.ts`, which spawns `src/child.ts` via `child_process.fork()` (inheriting `NODE_CHANNEL_FD` and tsx's `--import` loader). The child reads `src/data.txt` and sends its content over IPC so the parent writes `dist/out.txt`. Editing `src/data.txt` invalidates the cache.

## `vt run --cache run`

first run: cache miss

```
$ tsx src/parent.ts
```

## `vtt print-file dist/out.txt`

```
ipc:initial-tsx-ipc
```

## `vt run --cache run`

second run: cache hit

```
$ tsx src/parent.ts ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/data.txt initial-tsx-ipc edited-tsx-ipc`

```
```

## `vt run --cache run`

third run: cache miss after editing src/data.txt

```
$ tsx src/parent.ts ○ cache miss: 'src/data.txt' modified, executing
```

## `vtt print-file dist/out.txt`

```
ipc:edited-tsx-ipc
```

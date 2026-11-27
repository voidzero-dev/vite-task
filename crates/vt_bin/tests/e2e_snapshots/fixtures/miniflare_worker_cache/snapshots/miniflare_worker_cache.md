# miniflare_worker_cache

`miniflare` (`workerd`) loads `src/worker.mjs` and its imported module `src/data.mjs`, serves a request, and writes the response to `dist/out.txt`. Editing `src/data.mjs` invalidates the cache.

## `vt run --cache run`

first run: cache miss

```
$ node run.mjs
```

## `vtt print-file dist/out.txt`

```
worker:initial-worker
```

## `vt run --cache run`

second run: cache hit

```
$ node run.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/data.mjs initial-worker edited-worker`

```
```

## `vt run --cache run`

third run: cache miss after editing src/data.mjs

```
$ node run.mjs ○ cache miss: 'src/data.mjs' modified, executing
```

## `vtt print-file dist/out.txt`

```
worker:edited-worker
```

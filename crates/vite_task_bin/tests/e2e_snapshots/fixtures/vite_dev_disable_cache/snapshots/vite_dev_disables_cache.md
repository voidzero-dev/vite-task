# vite_dev_disables_cache

`vt run --cache dev` brings up a Vite dev server programmatically on an ephemeral port and closes it immediately. Vite's `_createServer` calls `disableCache()` via `@voidzero-dev/vite-task-client`, so this run is never stored — the next invocation re-executes (cache miss / NotFound).

## `vt run --cache dev`

first run — Vite dev start calls disableCache

```
$ node dev.mjs
```

## `vt run --cache dev`

cache miss (NotFound) because the first run was not stored

```
$ node dev.mjs
```

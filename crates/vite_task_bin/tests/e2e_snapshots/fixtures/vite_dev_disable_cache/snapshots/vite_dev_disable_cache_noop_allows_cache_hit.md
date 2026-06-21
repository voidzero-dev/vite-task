# vite_dev_disable_cache_noop_allows_cache_hit

`vt run --cache dev` brings up a Vite dev server programmatically on an
ephemeral port and closes it immediately. Vite calls `disableCache()` via
`@voidzero-dev/vite-task-client`, but the client temporarily ignores that
request, so the next invocation hits the cache.

## `vt run --cache dev`

first run — Vite dev calls disableCache, currently ignored by the client

```
$ node dev.mjs
```

## `vt run --cache dev`

cache hit because disableCache is temporarily a no-op

```
$ node dev.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

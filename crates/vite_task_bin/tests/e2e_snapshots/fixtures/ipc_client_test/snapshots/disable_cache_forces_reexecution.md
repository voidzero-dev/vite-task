# disable_cache_forces_reexecution

Exercises `disableCache`. The tool asks the runner not to cache this run,
so the next invocation re-executes instead of hitting a prior entry.

## `vt run disable-cache`

first run — tool calls disableCache

```
$ node scripts/disable_cache.mjs
```

## `vt run disable-cache`

cache miss (NotFound) because nothing was cached

```
$ node scripts/disable_cache.mjs
```

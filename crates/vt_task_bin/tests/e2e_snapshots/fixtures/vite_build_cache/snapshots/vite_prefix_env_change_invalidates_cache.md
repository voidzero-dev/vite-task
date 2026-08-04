# vite_prefix_env_change_invalidates_cache

`VITE_CACHE_LABEL` is picked up by Vite's patched `loadEnv`, which asks the runner for every `VITE_*` env via `getEnvs(pattern, { tracked: true })`. Flipping its value between runs must invalidate the cache AND change the build output because Vite substitutes `import.meta.env.VITE_CACHE_LABEL` at build time.

## `VITE_CACHE_LABEL=cache-alpha vt run --cache build`

first run: cache-alpha label

```
$ vite build
```

## `vtt grep-file dist/assets/main.js cache-alpha`

cache-alpha label is in the bundle

```
dist/assets/main.js: found "cache-alpha"
```

## `vtt grep-file dist/assets/main.js cache-bravo`

cache-bravo label is not in the bundle yet

```
dist/assets/main.js: missing "cache-bravo"
```

## `VITE_CACHE_LABEL=cache-alpha vt run --cache build`

cache hit: VITE_CACHE_LABEL unchanged

```
$ vite build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `VITE_CACHE_LABEL=cache-bravo vt run --cache build`

cache miss: envs changed — VITE_CACHE_LABEL value changed

```
$ vite build ○ cache miss: env 'VITE_CACHE_LABEL' changed, executing
```

## `vtt grep-file dist/assets/main.js cache-alpha`

cache-alpha label is gone after the rebuild

```
dist/assets/main.js: missing "cache-alpha"
```

## `vtt grep-file dist/assets/main.js cache-bravo`

cache-bravo label is now in the bundle

```
dist/assets/main.js: found "cache-bravo"
```

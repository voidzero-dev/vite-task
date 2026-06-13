# vite_prefix_env_change_invalidates_cache

`VITE_MODE` is picked up by Vite's patched `loadEnv`, which asks the runner for every `VITE_*` env via `getEnvs(pattern, { tracked: true })`. Flipping its value between runs must invalidate the cache AND change the build output — Vite's `define` plugin substitutes `import.meta.env.VITE_MODE` at build time, so dead-code elimination leaves only the branch matching the value.

## `VITE_MODE=production vt run --cache build`

first run: production build

```
$ vite build
```

## `vtt grep-file dist/assets/main.js BUILD_MODE_PROD`

production build: PROD marker survived DCE

```
dist/assets/main.js: found "BUILD_MODE_PROD"
```

## `vtt grep-file dist/assets/main.js BUILD_MODE_DEV`

dev branch is gone

```
dist/assets/main.js: missing "BUILD_MODE_DEV"
```

## `VITE_MODE=production vt run --cache build`

cache hit: VITE_MODE unchanged

```
$ vite build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `VITE_MODE=development vt run --cache build`

cache miss: envs changed — VITE_MODE value changed

```
$ vite build ○ cache miss: env 'VITE_MODE' changed, executing
```

## `vtt grep-file dist/assets/main.js BUILD_MODE_PROD`

PROD marker gone after the dev rebuild

```
dist/assets/main.js: missing "BUILD_MODE_PROD"
```

## `vtt grep-file dist/assets/main.js BUILD_MODE_DEV`

DEV marker now in the bundle

```
dist/assets/main.js: found "BUILD_MODE_DEV"
```

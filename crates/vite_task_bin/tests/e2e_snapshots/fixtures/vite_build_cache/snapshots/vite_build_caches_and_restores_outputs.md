# vite_build_caches_and_restores_outputs

`vt run --cache build` must produce a cache hit on the second run without any manual input/output configuration. Vite reports `ignoreInput(outDir)` + `ignoreInput/Output(cacheDir)` via `@voidzero-dev/vite-task-client`, so fspy-detected reads of `dist/` and writes to `node_modules/.vite/` don't poison the cache.

## `vt run --cache build`

first run: cache miss, emits dist/

```
$ vite build
```

## `vtt stat-file dist/assets/main.js`

existence check — content would drift across Vite versions

```
dist/assets/main.js: exists
```

## `vtt rm dist/assets/main.js`

remove the artefact so the cache-hit restore is observable

```
```

## `vt run --cache build`

cache hit: outputs restored without manual config

```
$ vite build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file dist/assets/main.js`

restored from the cache archive

```
dist/assets/main.js: exists
```

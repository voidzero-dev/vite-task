# esbuild_cache

`esbuild` JS API spawns the Go `esbuild` child service to bundle `src/index.js` and `src/dep.js` into `dist/out.js`. Editing `src/dep.js` invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ node build.mjs
```

## `vtt grep-file dist/out.js initial-esbuild`

```
dist/out.js: found "initial-esbuild"
```

## `vt run --cache build`

second run: cache hit

```
$ node build.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/dep.js initial-esbuild edited-esbuild`

```
```

## `vt run --cache build`

third run: cache miss after editing src/dep.js

```
$ node build.mjs ○ cache miss: 'src/dep.js' modified, executing
```

## `vtt grep-file dist/out.js edited-esbuild`

```
dist/out.js: found "edited-esbuild"
```

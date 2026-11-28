# rspack_build_cache

`rspack build` (Rust N-API bundler with `mimalloc` + rayon) bundles `src/index.js` and `src/dep.js` into `dist/main.js`. Editing `src/dep.js` invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ rspack build
```

## `vtt grep-file dist/main.js initial-rspack`

```
dist/main.js: found "initial-rspack"
```

## `vt run --cache build`

second run: cache hit

```
$ rspack build ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/dep.js initial-rspack edited-rspack`

```
```

## `vt run --cache build`

third run: cache miss after editing src/dep.js

```
$ rspack build ○ cache miss: 'src/dep.js' modified, executing
```

## `vtt grep-file dist/main.js edited-rspack`

```
dist/main.js: found "edited-rspack"
```

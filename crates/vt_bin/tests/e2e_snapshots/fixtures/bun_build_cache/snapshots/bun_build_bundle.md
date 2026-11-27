# bun_build_bundle

`bun build` bundles `src/index.js` and `src/dep.js` into `dist/bundle.js`. Editing `src/dep.js` invalidates the cache.

## `vt run --cache bundle`

first run: cache miss

```
$ bun build src/index.js --outfile dist/bundle.js
Bundled 2 modules in <duration>

  bundle.js  <size>  (entry point)
```

## `vtt grep-file dist/bundle.js initial-bun`

```
dist/bundle.js: found "initial-bun"
```

## `vt run --cache bundle`

second run: cache hit

```
$ bun build src/index.js --outfile dist/bundle.js ◉ cache hit, replaying
Bundled 2 modules in <duration>

  bundle.js  <size>  (entry point)


---
vt run: cache hit.
```

## `vtt replace-file-content src/dep.js initial-bun edited-bun`

```
```

## `vt run --cache bundle`

third run: cache miss after editing src/dep.js

```
$ bun build src/index.js --outfile dist/bundle.js ○ cache miss: 'src/dep.js' modified, executing
Bundled 2 modules in <duration>

  bundle.js  <size>  (entry point)
```

## `vtt grep-file dist/bundle.js edited-bun`

```
dist/bundle.js: found "edited-bun"
```

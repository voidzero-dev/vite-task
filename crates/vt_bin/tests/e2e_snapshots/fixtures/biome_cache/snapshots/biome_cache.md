# biome_cache

`biome check` checks and lints `src/index.js`. Editing `src/index.js` invalidates the cache.

## `vt run --cache check`

first run: cache miss

```
$ biome check src/index.js
Checked 1 file in <duration>. No fixes applied.
```

## `vt run --cache check`

second run: cache hit

```
$ biome check src/index.js ◉ cache hit, replaying
Checked 1 file in <duration>. No fixes applied.

---
vt run: cache hit.
```

## `vtt replace-file-content src/index.js initial-biome edited-biome`

```
```

## `vt run --cache check`

third run: cache miss after editing src/index.js

```
$ biome check src/index.js ○ cache miss: 'src/index.js' modified, executing
Checked 1 file in <duration>. No fixes applied.
```

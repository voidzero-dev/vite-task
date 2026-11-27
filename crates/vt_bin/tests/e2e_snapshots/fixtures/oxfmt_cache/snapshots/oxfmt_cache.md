# oxfmt_cache

`oxfmt --check` formats-checks `src/index.js`. Editing `src/index.js` invalidates the cache.

## `vt run --cache format-check`

first run: cache miss

```
$ oxfmt --check src/index.js
Checking formatting...

All matched files use the correct format.
Finished in <duration> on 1 files using <n> threads.
```

## `vt run --cache format-check`

second run: cache hit

```
$ oxfmt --check src/index.js ◉ cache hit, replaying
Checking formatting...

All matched files use the correct format.
Finished in <duration> on 1 files using <n> threads.

---
vt run: cache hit.
```

## `vtt replace-file-content src/index.js initial-oxfmt edited-oxfmt`

```
```

## `vt run --cache format-check`

third run: cache miss after editing src/index.js

```
$ oxfmt --check src/index.js ○ cache miss: 'src/index.js' modified, executing
Checking formatting...

All matched files use the correct format.
Finished in <duration> on 1 files using <n> threads.
```

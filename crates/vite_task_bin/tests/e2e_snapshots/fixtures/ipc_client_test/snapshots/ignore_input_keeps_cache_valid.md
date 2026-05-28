# ignore_input_keeps_cache_valid

Exercises `ignoreInput` through `@voidzero-dev/vite-task-client`.
The runner treats `cache_like/` as non-input, so mutations to it between
runs do not invalidate the cache.

## `vt run ignore-input`

populate the cache

```
$ node scripts/ignore_input.mjs
```

## `vtt write-file cache_like/other.txt after`

mutate the ignored directory — would invalidate if tracked

```
```

## `vt run ignore-input`

cache hit: cache_like/ was ignored via ignoreInput

```
$ node scripts/ignore_input.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

# ignore_input_keeps_cache_valid

Exercises `ignoreInput` through `@voidzero-dev/vite-task-client`. The runner treats `cache_like/` as non-input for auto-inferred reads, but declared inputs under that path stay in the configured cache key.

## `vtt write-file cache_like/input.txt before`

seed the file the task will read and ignore

```
```

## `vt run ignore-input`

populate the cache

```
$ node scripts/ignore_input.mjs
before
```

## `vtt write-file cache_like/input.txt after`

mutate the ignored input — would invalidate if tracked

```
```

## `vt run ignore-input`

cache hit: cache_like/ was ignored via ignoreInput

```
$ node scripts/ignore_input.mjs ◉ cache hit, replaying
before

---
vt run: cache hit.
```

## `vtt write-file cache_like/input.txt manual-before`

seed the declared input for input: [cache_like/input.txt]

```
```

## `vt run ignore-input-manual-input`

populate the cache with the manual-only input fingerprint

```
$ node scripts/ignore_input.mjs
manual-before
```

## `vtt write-file cache_like/input.txt manual-after`

mutate the declared input for input: [cache_like/input.txt]

```
```

## `vt run ignore-input-manual-input`

cache miss: manual input config wins over ignoreInput

```
$ node scripts/ignore_input.mjs ○ cache miss: 'cache_like/input.txt' modified, executing
manual-after
```

## `vtt write-file cache_like/input.txt auto-manual-before`

seed the declared input for input: [{ auto: true }, cache_like/input.txt]

```
```

## `vt run ignore-input-auto-manual-input`

populate the cache with the auto-plus-manual input fingerprint

```
$ node scripts/ignore_input.mjs
auto-manual-before
```

## `vtt write-file cache_like/input.txt auto-manual-after`

mutate the declared input for input: [{ auto: true }, cache_like/input.txt]

```
```

## `vt run ignore-input-auto-manual-input`

cache miss: explicit input still wins when auto inference is also enabled

```
$ node scripts/ignore_input.mjs ○ cache miss: 'cache_like/input.txt' modified, executing
auto-manual-after
```

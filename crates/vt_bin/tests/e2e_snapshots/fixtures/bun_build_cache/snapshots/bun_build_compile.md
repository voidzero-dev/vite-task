# bun_build_compile

`bun build --compile` builds a standalone executable and runs it to read `src/data.txt` and write `dist/out.txt`. Editing `src/data.txt` invalidates the cache.

## `vt run --cache compile-and-run`

first run: cache miss

```
$ bun run scripts/compile-and-run.js
```

## `vtt print-file dist/out.txt`

```
bun-compiled:initial-compiled
```

## `vt run --cache compile-and-run`

second run: cache hit

```
$ bun run scripts/compile-and-run.js ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/data.txt initial-compiled edited-compiled`

```
```

## `vt run --cache compile-and-run`

third run: cache miss after editing src/data.txt

```
$ bun run scripts/compile-and-run.js ○ cache miss: 'src/data.txt' modified, executing
```

## `vtt print-file dist/out.txt`

```
bun-compiled:edited-compiled
```

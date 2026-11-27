# deno_run_cache

`deno run --no-remote` spawns a child process via `Deno.execPath()` (exercising `/proc/self/exe` virtualization and recursive exec) and a Web Worker reading a separate input file. Editing either input invalidates the cache.

## `vt run --cache build`

first run: cache miss

```
$ deno run --quiet --no-remote --allow-read --allow-write --allow-run src/main.ts
```

## `vtt print-file dist/out.txt`

```
child=initial-child;worker=initial-worker
```

## `vt run --cache build`

second run: cache hit

```
$ deno run --quiet --no-remote --allow-read --allow-write --allow-run src/main.ts ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content src/child-data.txt initial-child edited-child`

```
```

## `vt run --cache build`

third run: cache miss after editing child process input

```
$ deno run --quiet --no-remote --allow-read --allow-write --allow-run src/main.ts ○ cache miss: 'src/child-data.txt' modified, executing
```

## `vtt print-file dist/out.txt`

```
child=edited-child;worker=initial-worker
```

## `vtt replace-file-content src/worker-data.txt initial-worker edited-worker`

```
```

## `vt run --cache build`

fourth run: cache miss after editing Web Worker input

```
$ deno run --quiet --no-remote --allow-read --allow-write --allow-run src/main.ts ○ cache miss: 'src/worker-data.txt' modified, executing
```

## `vtt print-file dist/out.txt`

```
child=edited-child;worker=edited-worker
```

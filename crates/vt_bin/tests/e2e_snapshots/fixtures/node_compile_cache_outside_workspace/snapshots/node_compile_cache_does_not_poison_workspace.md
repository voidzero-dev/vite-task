# node_compile_cache_does_not_poison_workspace

Runs a small Node script that turns on Node's compile cache. The cache should land in the OS temp directory (outside the workspace), so two `vt run --cache build` calls should be a miss then a hit, and the workspace should then hold only the fixture's files and the task cache in `node_modules`. On Windows, if the spawned task env doesn't have `LOCALAPPDATA` (or `TMP`/`TEMP`), Node's temp directory fallback can resolve relative to the cwd and put the cache inside the workspace, where the final listing shows it.

## `vt run --cache build`

first run: cache miss

```
$ node run.mjs
done
```

## `vt run --cache build`

second run: cache hit

```
$ node run.mjs ◉ cache hit, replaying
done

---
vt run: cache hit.
```

## `vtt list-dir .`

no compile cache folder in the workspace

```
a.mjs
b.mjs
c.mjs
node_modules
package.json
run.mjs
snapshots
snapshots.toml
vite-task.json
```

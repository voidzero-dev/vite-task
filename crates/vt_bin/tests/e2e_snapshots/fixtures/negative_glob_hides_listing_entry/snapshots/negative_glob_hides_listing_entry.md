# negative_glob_hides_listing_entry

The task lists the package root and prints its entries, with `"!out/**"` in its `input`.

## `vt run build`

```
$ vtt list-dir .
node_modules
package.json
snapshots
snapshots.toml
vite-task.json
```

## `vtt mkdir out`

an `out` folder, which `!out/**` excludes, appears in the listed folder

```
```

## `vt run build`

```
$ vtt list-dir . ◉ cache hit, replaying
node_modules
package.json
snapshots
snapshots.toml
vite-task.json

---
vt run: cache hit.
```

## `vtt write-file notes.txt x`

a file the exclusion doesn't match appears in the listed folder

```
```

## `vt run build`

```
$ vtt list-dir . ○ cache miss: 'notes.txt' added in workspace root, executing
node_modules
notes.txt
out
package.json
snapshots
snapshots.toml
vite-task.json
```

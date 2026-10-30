# dist_appears_in_listed_folder

The task lists the package root and prints its entries, like a tool looking for the files to process.

## `vt run build`

```
$ vtt list-dir .
node_modules
package.json
snapshots
snapshots.toml
vite-task.json
```

## `vtt mkdir dist`

a `dist` folder the task didn't make appears in the listed folder

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

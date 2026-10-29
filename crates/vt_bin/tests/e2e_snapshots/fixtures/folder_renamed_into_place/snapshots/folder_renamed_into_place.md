# folder_renamed_into_place

The task writes `tmp/a.js`, renames `tmp` to `deps` and reads `deps/a.js` back, like Vite's dependency optimizer renaming `deps_temp_*` to `deps`.

## `vt run build`

```
$ vtt fs mkdir tmp write tmp/a.js x rename tmp deps read deps/a.js
```

## `vtt rm -r deps`

remove the renamed folder, as on a clean checkout

```
```

## `vt run build`

```
$ vtt fs mkdir tmp write tmp/a.js x rename tmp deps read deps/a.js ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file deps/a.js`

```
deps/a.js: exists
```

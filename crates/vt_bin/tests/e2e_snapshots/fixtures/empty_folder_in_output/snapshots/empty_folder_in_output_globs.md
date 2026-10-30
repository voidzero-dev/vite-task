# empty_folder_in_output_globs

The same task with explicit `output` globs, `out/**`, which match `out/empty`.

## `vt run build-globs`

```
$ vtt fs mkdir out/empty write out/a.txt hi stat out
```

## `vtt rm -r out`

remove the output folder, as on a clean checkout

```
```

## `vt run build-globs`

```
$ vtt fs mkdir out/empty write out/a.txt hi stat out ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file out/empty`

```
out/empty: exists
```

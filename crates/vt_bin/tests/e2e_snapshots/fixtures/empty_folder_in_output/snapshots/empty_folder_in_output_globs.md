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

on Windows, where a mkdir is recorded as a read, `out/empty` is an input too, and vt names whichever removed input it finds first, so the miss reason is redacted

```
$ vtt fs mkdir out/empty write out/a.txt hi stat out ○ cache miss: <removed input>, executing
```

## `vtt stat-file out/empty`

```
out/empty: exists
```

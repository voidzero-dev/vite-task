# output_globs___files_restored_on_cache_hit

With explicit output globs (`dist/**`), the first run writes a file to
`dist/`. After deleting `dist/`, a second run with no input changes is a
cache hit and the archived output file is restored.

## `vt run build`

first run — cache miss, writes dist/output.txt

```
$ vtt write-file dist/output.txt built
```

## `vtt print-file dist/output.txt`

file is on disk after the run

```
built
```

## `vtt rm -rf dist`

delete dist/ to prove the restore is real

```
```

## `vt run build`

second run — cache hit, restores from archive

```
$ vtt write-file dist/output.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/output.txt`

file restored from archive

```
built
```

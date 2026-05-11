# output_globs___negative_excludes_files_from_archive

A file matched by a negative output glob is not archived, so it is not
restored on cache hit.

## `vt run build-with-negative`

first run — writes dist/keep.txt and dist/skip.txt

```
$ vtt write-file dist/keep.txt keep

$ vtt write-file dist/skip.txt skip

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/keep.txt`

keep.txt was written

```
keep
```

## `vtt print-file dist/skip.txt`

skip.txt was written

```
skip
```

## `vtt rm -rf dist`

delete dist/ to prove the restore is real

```
```

## `vt run build-with-negative`

second run — cache hit, restores from archive

```
$ vtt write-file dist/keep.txt keep ◉ cache hit, replaying

$ vtt write-file dist/skip.txt skip ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/keep.txt`

keep.txt restored from archive

```
keep
```

## `vtt print-file dist/skip.txt`

skip.txt is NOT restored — it was excluded by the negative glob

```
dist/skip.txt: not found
```

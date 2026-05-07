# output_globs___negative_excludes_files_from_archive

A file matched by a negative output glob is not archived, so it is not
restored on cache hit.

## `vt run build-with-negative`

```
$ vtt write-file dist/keep.txt keep

$ vtt write-file dist/skip.txt skip

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/keep.txt`

```
keep
```

## `vtt print-file dist/skip.txt`

```
skip
```

## `vtt rm -rf dist`

```
```

## `vt run build-with-negative`

```
$ vtt write-file dist/keep.txt keep ◉ cache hit, replaying

$ vtt write-file dist/skip.txt skip ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/keep.txt`

```
keep
```

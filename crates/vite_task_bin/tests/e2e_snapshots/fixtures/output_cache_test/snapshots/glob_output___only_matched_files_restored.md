# glob_output___only_matched_files_restored

Glob output: only files matching the configured output globs are restored
on a cache hit; files produced outside those globs are left alone.

## `vt run glob-output`

first run writes dist/out.txt and tmp/temp.txt

```
$ vtt write-file dist/out.txt built

$ vtt write-file tmp/temp.txt temp

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt rm dist/out.txt`

delete the glob-matched output

```
```

## `vtt rm tmp/temp.txt`

delete the non-matched output

```
```

## `vt run glob-output`

cache hit: should restore dist/out.txt but not tmp/temp.txt

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

$ vtt write-file tmp/temp.txt temp ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/out.txt`

dist/out.txt was restored

```
built
```

## `vtt print-file tmp/temp.txt`

should fail - tmp not in output globs

```
tmp/temp.txt: not found
```

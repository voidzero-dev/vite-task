# negative_output___excluded_files_not_restored

Negative output globs exclude matching files from the cache archive, so
they are not restored on a cache hit even though the task produced them.

## `vt run negative-output`

first run writes dist/out.txt and dist/cache/tmp.txt

```
$ vtt write-file dist/out.txt built

$ vtt write-file dist/cache/tmp.txt temp

---
vt run: 0/2 cache hit (0%). (Run `vt run --last-details` for full details)
```

## `vtt rm dist/out.txt`

delete the archived output

```
```

## `vtt rm dist/cache/tmp.txt`

delete the excluded output

```
```

## `vt run negative-output`

cache hit: should restore dist/out.txt but NOT dist/cache/tmp.txt

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

$ vtt write-file dist/cache/tmp.txt temp ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%). (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/out.txt`

dist/out.txt was restored

```
built
```

## `vtt print-file dist/cache/tmp.txt`

should fail - excluded by !dist/cache/**

```
dist/cache/tmp.txt: not found
```

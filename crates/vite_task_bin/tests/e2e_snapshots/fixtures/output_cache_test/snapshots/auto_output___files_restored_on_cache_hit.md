# auto_output___files_restored_on_cache_hit

Auto output detection (default): output files written by the task are
restored on a cache hit.

## `vt run auto-output`

first run writes dist/out.txt

```
$ vtt write-file dist/out.txt built
```

## `vtt print-file dist/out.txt`

output exists after the build

```
built
```

## `vtt rm dist/out.txt`

delete only the file (keep dir to avoid an fspy inferred-input miss on Windows)

```
```

## `vt run auto-output`

cache hit, should restore dist/out.txt

```
$ vtt write-file dist/out.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file dist/out.txt`

output was restored from the archive

```
built
```

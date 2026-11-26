# ignore_output_keeps_write_only_file_out_of_listing

The task lists the workspace and writes `scratch.txt` without reading it. It reports the file with `ignoreOutput`, so a hit leaves the file missing.

## `vt run write-only`

```
$ node write-only.mjs
```

## `vtt rm scratch.txt`

```
```

## `vt run write-only`

```
$ node write-only.mjs ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file scratch.txt`

```
scratch.txt: missing
```

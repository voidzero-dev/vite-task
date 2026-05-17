# output_globs___old_archive_removed_on_rewrite

When a cached task re-runs (cache miss because an input changed), it writes a new archive and the previous archive file is cleaned up. After two cache-missing runs of the same task the cache directory still contains only one `.tar.zst` archive.

## `vt run build`

first run — cache miss, writes archive A

```
$ vtt write-file dist/output.txt built
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst`

exactly one archive on disk

```
<uuid>.tar.zst
```

## `vtt write-file src/main.ts changed`

modify an input so the next run is a cache miss

```
```

## `vt run build`

second run — cache miss, writes archive B and removes A

```
$ vtt write-file dist/output.txt built ○ cache miss: 'src/main.ts' modified, executing
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst`

still exactly one archive — A was cleaned up

```
<uuid>.tar.zst
```

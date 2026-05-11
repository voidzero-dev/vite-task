# output_globs___old_archive_removed_on_rewrite

When a cached task re-runs (cache miss because an input changed), it
writes a new archive and the previous archive file is cleaned up. After
two runs of the same task the cache directory still contains only one
\".tar.zst\" file.

## `vt run build`

```
$ vtt write-file dist/output.txt built
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst`

```
<uuid>.tar.zst
```

## `vtt write-file src/main.ts changed`

```
```

## `vt run build`

```
$ vtt write-file dist/output.txt built ○ cache miss: 'src/main.ts' modified, executing
```

## `vtt list-dir node_modules/.vite/task-cache --ext .tar.zst`

```
<uuid>.tar.zst
```

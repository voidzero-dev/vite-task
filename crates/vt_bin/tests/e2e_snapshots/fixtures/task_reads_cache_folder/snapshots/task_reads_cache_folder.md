# task_reads_cache_folder

The task lists every folder under `node_modules/.vite`, the cache's own folder among them, like a tool crawling the project, and writes `out.txt`. The cache folder changes after every run: the run's archive is added, the previous one is removed, and the run summary is written.

## `vt run crawl`

no cache entry yet

```
$ vtt fs walk node_modules/.vite write out.txt built
```

## `vt run crawl`

nothing changed

```
$ vtt fs walk node_modules/.vite write out.txt built ○ cache miss: '<uuid>.tar.zst' added in or removed from 'node_modules/.vite/task-cache/v<n>', executing
```

## `vt run crawl`

nothing changed

```
$ vtt fs walk node_modules/.vite write out.txt built ○ cache miss: '<uuid>.tar.zst' added in or removed from 'node_modules/.vite/task-cache/v<n>', executing
```

## `vtt write-file node_modules/.vite/other.txt x`

a file appears in a folder the task lists, outside the cache

```
```

## `vt run crawl`

two inputs changed, and either may be named

```
$ vtt fs walk node_modules/.vite write out.txt built ○ cache miss: <changed input>, executing
```

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
$ vtt fs walk node_modules/.vite write out.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vt run crawl`

nothing changed

```
$ vtt fs walk node_modules/.vite write out.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt write-file node_modules/.vite/other.txt x`

a file appears in a folder the task lists, outside the cache

```
```

## `vt run crawl`

```
$ vtt fs walk node_modules/.vite write out.txt built ○ cache miss: 'other.txt' added in 'node_modules/.vite', executing
```

# dry_run_writes_the_cache_folder

Bug: a dry run should write nothing, but it creates the cache folder and its database, even when no step is cached.

## `vtt stat-file node_modules/.vite/task-cache`

Nothing has run yet.

```
node_modules/.vite/task-cache: missing
```

## `vt run --dry-run uncached`

Caching is off for this task, so there's no cache to check.

```
$ vtt print uncached ⊘ cache disabled
```

## `vtt stat-file node_modules/.vite/task-cache`

Expected: missing. It was created by the dry run.

```
node_modules/.vite/task-cache: exists
```

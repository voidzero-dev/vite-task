# task_with_cache_enabled

A task with caching enabled should produce a cache hit on the second run.

## `vt run cached-task`

cache miss

```
$ vtt print-file test.txt
test content
```

## `vt run cached-task`

cache hit

```
$ vtt print-file test.txt ◉ cache hit, replaying
test content

---
vt run: cache hit.
```

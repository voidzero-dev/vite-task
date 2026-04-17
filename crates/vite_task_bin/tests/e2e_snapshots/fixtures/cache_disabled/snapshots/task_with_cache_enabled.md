# task_with_cache_enabled

Tests that cache: false in task config disables caching

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

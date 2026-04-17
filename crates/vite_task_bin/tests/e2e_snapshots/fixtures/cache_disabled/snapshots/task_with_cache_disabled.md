# task_with_cache_disabled

Tests that cache: false in task config disables caching

## `vt run no-cache-task`

cache miss

```
$ vtt print-file test.txt ⊘ cache disabled
test content
```

## `vt run no-cache-task`

cache disabled, runs again

```
$ vtt print-file test.txt ⊘ cache disabled
test content
```

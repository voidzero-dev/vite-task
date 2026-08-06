# task_with_cache_disabled

A task configured with `cache: false` should re-run on every invocation instead of being cached.

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

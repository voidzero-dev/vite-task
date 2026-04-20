# env_removed

Unsetting a tracked env var that was previously set should invalidate the cache.

## `MY_ENV=1 vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `vt run test`

cache miss: env removed

```
$ vtt print-file test.txt ○ cache miss: envs changed, executing
initial content
```

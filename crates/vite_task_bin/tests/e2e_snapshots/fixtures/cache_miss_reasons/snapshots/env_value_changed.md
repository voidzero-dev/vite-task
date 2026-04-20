# env_value_changed

Changing the value of a tracked env var between runs should invalidate the cache.

## `MY_ENV=1 vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `MY_ENV=2 vt run test`

cache miss: env value changed

```
$ vtt print-file test.txt ○ cache miss: envs changed, executing
initial content
```

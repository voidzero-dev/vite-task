# untracked_env_added

Adding an `untrackedEnv` entry to the task config should invalidate the cache.

## `vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `vtt replace-file-content vite-task.json '"cache": true' '"cache": true, "untrackedEnv": ["MY_UNTRACKED"]'`

add untracked env

```
```

## `vt run test`

cache miss: untracked env added

```
$ vtt print-file test.txt ○ cache miss: untracked env config changed, executing
initial content
```

# untracked_env_removed

Removing an existing `untrackedEnv` entry from the task config should invalidate the cache.

## `vtt replace-file-content vite-task.json '"cache": true' '"cache": true, "untrackedEnv": ["MY_UNTRACKED"]'`

setup

```
```

## `vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `vtt replace-file-content vite-task.json '"cache": true, "untrackedEnv": ["MY_UNTRACKED"]' '"cache": true'`

remove untracked env

```
```

## `vt run test`

cache miss: untracked env removed

```
$ vtt print-file test.txt ○ cache miss: untracked env config changed, executing
initial content
```

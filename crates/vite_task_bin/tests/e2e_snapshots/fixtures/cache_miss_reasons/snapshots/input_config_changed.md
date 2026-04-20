# input_config_changed

Changing the task's `input` configuration should invalidate the cache even when the underlying files are unchanged.

## `vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `vtt replace-file-content vite-task.json '"cache": true' '"cache": true, "input": ["test.txt"]'`

change input config

```
```

## `vt run test`

cache miss: configuration changed

```
$ vtt print-file test.txt ○ cache miss: input configuration changed, executing
initial content
```

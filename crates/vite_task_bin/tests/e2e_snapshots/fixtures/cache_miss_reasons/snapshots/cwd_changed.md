# cwd_changed

Changing the task's `cwd` should invalidate the cache, even if the input file exists at the new location.

## `vt run test`

cache miss

```
$ vtt print-file test.txt
initial content
```

## `vtt mkdir -p subfolder`

```
```

## `vtt cp test.txt subfolder/test.txt`

```
```

## `vtt replace-file-content vite-task.json '"cache": true' '"cache": true, "cwd": "subfolder"'`

change cwd

```
```

## `vt run test`

cache miss: cwd changed

```
~/subfolder$ vtt print-file test.txt ○ cache miss: working directory changed, executing
initial content
```

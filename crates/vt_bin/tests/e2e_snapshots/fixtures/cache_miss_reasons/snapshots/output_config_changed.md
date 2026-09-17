# output_config_changed

Changing only the task's output configuration causes a cache miss. Restoring the original configuration reuses its existing entry.

## `vt run test`

populate the original entry

```
$ vtt print-file test.txt
initial content
```

## `vt run test`

cache hit with the original output config

```
$ vtt print-file test.txt ◉ cache hit, replaying
initial content

---
vt run: cache hit.
```

## `vtt replace-file-content vite-task.json '"env": ["MY_ENV"]' '"env": ["MY_ENV"], "output": []'`

change only the output config

```
```

## `vt run test`

cache miss: output configuration changed

```
$ vtt print-file test.txt ○ cache miss: output configuration changed, executing
initial content
```

## `vt run test`

cache hit with the new output config

```
$ vtt print-file test.txt ◉ cache hit, replaying
initial content

---
vt run: cache hit.
```

## `vtt replace-file-content vite-task.json '"env": ["MY_ENV"], "output": []' '"env": ["MY_ENV"]'`

restore the original output config

```
```

## `vt run test`

cache hit: the original entry is still available

```
$ vtt print-file test.txt ◉ cache hit, replaying
initial content

---
vt run: cache hit.
```

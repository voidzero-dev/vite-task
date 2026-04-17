# read_file_with_colon_in_name

Tests that a task name with colon works correctly with caching

## `vt run read_colon_in_name`

cache miss

```
$ vtt print-file node:fs
node:fs: not found
```

## `vt run read_colon_in_name`

cache hit

```
$ vtt print-file node:fs ◉ cache hit, replaying
node:fs: not found

---
vt run: cache hit.
```

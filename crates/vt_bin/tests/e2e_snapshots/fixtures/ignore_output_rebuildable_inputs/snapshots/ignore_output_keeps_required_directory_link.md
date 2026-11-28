# ignore_output_keeps_required_directory_link

The task lists a directory link, recreates it, and writes a file through it. Deleting the tracked link makes the task rerun.

## `node read-link.mjs`

```
```

## `vt run read-link`

```
$ node read-link.mjs
```

## `vtt rm -r alias result.txt`

```
```

## `vt run read-link`

```
$ node read-link.mjs ○ cache miss: 'alias' removed from workspace root, executing
```

## `vtt print-file result.txt`

```
missing
```

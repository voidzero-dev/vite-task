# rewrite_own_file

The task reads `state.txt` and writes it back, like a tool that reads and rewrites its cache file.

## `vt run rewrite`

no state.txt yet: the task probes for it and writes it

```
$ vtt fs read state.txt write state.txt v1
```

## `vt run rewrite`

state.txt is as the task left it

```
$ vtt fs read state.txt write state.txt v1 ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt rm state.txt`

remove the file the task wrote, as on a clean checkout

```
```

## `vt run rewrite`

```
$ vtt fs read state.txt write state.txt v1 ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file state.txt`

```
v1
```

## `vtt write-file state.txt garbage`

a file that is there must be as the task left it

```
```

## `vt run rewrite`

```
$ vtt fs read state.txt write state.txt v1 ○ cache miss: 'state.txt' modified, executing
```

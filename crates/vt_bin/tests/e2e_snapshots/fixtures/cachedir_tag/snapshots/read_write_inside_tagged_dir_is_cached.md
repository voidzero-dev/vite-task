# read_write_inside_tagged_dir_is_cached

A directory holding a valid `CACHEDIR.TAG` is a cache directory. A task that reads and rewrites a file anywhere inside it is cached, and changing that file afterwards doesn't cause a cache miss.

## `vt run tagged`

```
$ vtt replace-file-content tagged/sub/state.txt i !
```

## `vt run tagged`

cache hit

```
$ vtt replace-file-content tagged/sub/state.txt i ! ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt replace-file-content tagged/sub/state.txt i x`

```
```

## `vt run tagged`

still a cache hit: the file is inside the tagged directory

```
$ vtt replace-file-content tagged/sub/state.txt i ! ◉ cache hit, replaying

---
vt run: cache hit.
```

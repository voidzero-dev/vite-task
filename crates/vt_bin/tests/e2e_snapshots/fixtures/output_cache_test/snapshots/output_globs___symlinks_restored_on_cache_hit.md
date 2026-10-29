# output_globs___symlinks_restored_on_cache_hit

A symbolic link matched by an output glob is archived as a link, so a cache hit restores it. Restoring also replaces a link that is already there. Not run on Windows, where a symbolic link in a task's outputs can't be cached.

## `vt run build-with-symlink`

first run — cache miss, writes dist/store/pkg/index.js and links dist/pkg to store/pkg

```
$ vtt write-file dist/store/pkg/index.js built

$ vtt symlink store/pkg dist/pkg

---
vt run: 0/2 cache hit (0%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt rm -rf dist`

delete dist/ to prove the restore is real

```
```

## `vt run build-with-symlink`

second run — cache hit, restores the file and the link

```
$ vtt write-file dist/store/pkg/index.js built ◉ cache hit, replaying

$ vtt symlink store/pkg dist/pkg ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/pkg/index.js`

the file is reachable through the restored link

```
built
```

## `vt run build-with-symlink`

third run — cache hit, restores over the link the second run left

```
$ vtt write-file dist/store/pkg/index.js built ◉ cache hit, replaying

$ vtt symlink store/pkg dist/pkg ◉ cache hit, replaying

---
vt run: 2/2 cache hit (100%), 2/2 successful. (Run `vt run --last-details` for full details)
```

## `vtt print-file dist/pkg/index.js`

the file is still reachable through the link

```
built
```

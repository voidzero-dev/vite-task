# writes_inside_tagged_dir_are_still_restored

The tag only affects inputs. A file the task writes inside a tagged directory is still an output, so a cache hit restores it.

## `vt run tagged-write`

```
$ vtt write-file tagged/out/built.txt built
```

## `vtt rm -rf tagged/out`

delete the output

```
```

## `vt run tagged-write`

cache hit, restores the output

```
$ vtt write-file tagged/out/built.txt built ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file tagged/out/built.txt`

restored

```
built
```

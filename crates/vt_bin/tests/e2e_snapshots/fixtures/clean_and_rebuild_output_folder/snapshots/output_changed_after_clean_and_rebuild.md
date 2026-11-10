# output_changed_after_clean_and_rebuild

The task removes `out` with `out/a.txt` in it and writes `out/a.txt` again, then a later step changes `out/a.txt`. Removing the file doesn't read it, so the change doesn't make the next run miss.

## `vtt write-file out/a.txt stale`

an earlier build left `out/a.txt`

```
```

## `vt run build`

```
$ vtt fs read src/a.txt list out rm out mkdir out write out/a.txt hi stat out
```

## `vtt write-file out/a.txt stamped`

an uncached step changes the output, like a step stamping a version into it

```
```

## `vt run build`

```
$ vtt fs read src/a.txt list out rm out mkdir out write out/a.txt hi stat out ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt print-file out/a.txt`

```
hi
```

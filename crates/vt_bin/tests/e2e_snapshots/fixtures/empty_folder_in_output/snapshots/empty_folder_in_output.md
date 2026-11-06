# empty_folder_in_output

The task makes `out/empty` and leaves it empty, writes `out/a.txt` and stats `out`, like a build tool that makes a folder for outputs it doesn't have this time.

## `vt run build`

```
$ vtt fs mkdir out/empty write out/a.txt hi stat out
```

## `vtt rm -r out`

remove the output folder, as on a clean checkout

```
```

## `vt run build`

```
$ vtt fs mkdir out/empty write out/a.txt hi stat out ◉ cache hit, replaying

---
vt run: cache hit.
```

## `vtt stat-file out/empty`

```
out/empty: exists
```
